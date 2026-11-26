use crate::{
    config::*,
    contract,
    edi::{self, Finding},
    interchange::{self, AckInfo, Interchange, Outgoing},
    script,
    store::Store,
};
use anyhow::{Context, Result, anyhow, ensure};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File},
    path::Path,
    sync::Arc,
    time::Duration,
};

pub trait Clock: Send + Sync {
    fn millis(&self) -> i64;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn millis(&self) -> i64 {
        Utc::now().timestamp_millis()
    }
}

#[derive(Clone, Debug)]
pub struct Delivery {
    pub id: i64,
    pub channel: SendChannel,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Receipt {
    Accepted,
    Error,
    Slow,
    Drop,
}
#[derive(Clone)]
struct Task {
    execution: i64,
    root: Option<i64>,
    partner: usize,
    inbound: Option<Arc<Interchange>>,
    actions: VecDeque<Action>,
    faults: Vec<Fault>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct TrafficRow {
    pub id: i64,
    pub root: i64,
    pub time: i64,
    pub outgoing: bool,
    pub partner: String,
    pub kind: String,
    pub reference: String,
    pub version: String,
    pub status: String,
    pub attention: bool,
}

pub fn local_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|v| {
            v.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

/// The same literal matcher drives result inclusion and the desktop highlights.
pub struct TrafficSearch(regex::Regex);
impl TrafficSearch {
    pub fn new(text: &str) -> Result<Self> {
        Ok(Self(
            regex::RegexBuilder::new(&regex::escape(text))
                .case_insensitive(true)
                .build()?,
        ))
    }

    pub fn ranges<'a>(
        &'a self,
        text: &'a str,
    ) -> impl Iterator<Item = std::ops::Range<usize>> + 'a {
        self.0.find_iter(text).map(|m| m.range())
    }
}
impl TrafficRow {
    pub fn direction_label(&self) -> &'static str {
        if self.outgoing { "OUT" } else { "IN" }
    }
    pub fn status_label(&self) -> String {
        if self.attention {
            format!("⚠ {}", self.status)
        } else {
            self.status.clone()
        }
    }
    fn matches(&self, query: &TrafficSearch) -> bool {
        [
            &self.partner,
            &self.kind,
            &self.reference,
            self.direction_label(),
            &self.status_label(),
            &local_time(self.time),
        ]
        .iter()
        .any(|s| query.0.is_match(s))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryPeriod {
    Minute,
    Hour,
    Day,
    Week,
}

impl HistoryPeriod {
    fn milliseconds(self) -> i64 {
        match self {
            Self::Minute => 60_000,
            Self::Hour => 3_600_000,
            Self::Day => 86_400_000,
            Self::Week => 7 * 86_400_000,
        }
    }
}

pub struct Engine {
    pub workspace: Workspace,
    store: Store,
    clock: Arc<dyn Clock>,
    _lock: File,
    pending: BTreeMap<(i64, u64), Task>,
    sequence: u64,
    deliveries: Vec<Delivery>,
    events: Vec<Value>,
}
impl Engine {
    pub fn open(workspace: Workspace, clock: Arc<dyn Clock>) -> Result<Self> {
        let dir = workspace.root.join(".vendredi");
        fs::create_dir_all(&dir)?;
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("engine.lock"))?;
        lock.try_lock().context("workspace is already in use")?;
        let store = Store::open(&dir.join("vendredi.db"))?;
        store.batch(include_str!("schema.sql"))?;
        // 0.1 databases predate the X12 functional-group column.
        if store.query("SELECT grp FROM messages LIMIT 0", ()).is_err() {
            store.exec(
                "ALTER TABLE messages ADD COLUMN grp TEXT NOT NULL DEFAULT ''",
                (),
            )?;
        }
        for p in &workspace.partners {
            store.exec(
                "INSERT OR IGNORE INTO partner_state VALUES (?,0,'{}')",
                [p.name.as_str()],
            )?;
        }
        let mut e = Self {
            workspace,
            store,
            clock,
            _lock: lock,
            pending: BTreeMap::new(),
            sequence: 0,
            deliveries: Vec::new(),
            events: Vec::new(),
        };
        for row in e
            .store
            .query("SELECT id,root FROM transfers WHERE status='pending'", ())?
        {
            let id = row.get::<i64>(0)?;
            let root = row.get::<i64>(1)?;
            e.store
                .exec("UPDATE transfers SET status='interrupted' WHERE id=?", [id])?;
            e.event("interrupted", Some(root), json!({"transfer":id}))?;
        }
        e.prune()?;
        Ok(e)
    }
    fn prune(&mut self) -> Result<()> {
        let cutoff = self.clock.millis() - i64::from(self.workspace.retention_days) * 86_400_000;
        self.store.transaction(|s| {
            Self::delete_exchanges(
                s,
                "SELECT root FROM transfers GROUP BY root HAVING MAX(time) < ?",
                [cutoff],
            )
        })
    }

    pub fn clear_history(&mut self, period: HistoryPeriod, partner: Option<&str>) -> Result<usize> {
        let end = self.clock.millis();
        let start = end.saturating_sub(period.milliseconds());
        let delayed: Vec<_> = self.pending.values().filter_map(|t| t.root).collect();
        let delayed = serde_json::to_string(&delayed)?;
        let roots = "SELECT root FROM transfers
            WHERE root NOT IN (SELECT value FROM json_each(?4))
            GROUP BY root
            HAVING MAX(time) BETWEEN ?1 AND ?2 AND MAX(status = 'pending') = 0
              AND (?3 IS NULL OR (MIN(partner) = ?3 AND MAX(partner) = ?3))";
        let params = (start, end, partner, delayed.as_str());
        self.store.transaction(|s| {
            let matching = s
                .query(roots, params)?
                .iter()
                .map(|row| row.get::<i64>(0))
                .collect::<turso::Result<Vec<_>>>()?;
            if !matching.is_empty() {
                let ids = serde_json::to_string(&matching)?;
                Self::delete_exchanges(s, "SELECT value FROM json_each(?)", [ids.as_str()])?;
            }
            Ok(matching.len())
        })
    }

    fn delete_exchanges(
        s: &Store,
        roots: &str,
        params: impl turso::IntoParams + Clone,
    ) -> Result<()> {
        let transfers = format!("SELECT id FROM transfers WHERE root IN ({roots})");
        for sql in [
            format!("DELETE FROM events WHERE json_extract(body,'$.root') IN ({roots})"),
            format!(
                "DELETE FROM events WHERE json_extract(body,'$.root') IS NULL AND json_extract(body,'$.execution') IN (SELECT id FROM executions WHERE root IN ({roots}))"
            ),
            format!("DELETE FROM findings WHERE transfer IN ({transfers})"),
            format!(
                "DELETE FROM messages WHERE interchange IN (SELECT id FROM interchanges WHERE transfer IN ({transfers}))"
            ),
            format!("DELETE FROM interchanges WHERE transfer IN ({transfers})"),
            format!("DELETE FROM executions WHERE root IN ({roots})"),
            format!("DELETE FROM transfers WHERE root IN ({roots})"),
        ] {
            s.exec(&sql, params.clone())?;
        }
        Ok(())
    }
    fn event(&mut self, kind: &str, root: Option<i64>, detail: Value) -> Result<()> {
        let mut v = detail;
        v["event"] = json!(kind);
        v["root"] = json!(root);
        v["time"] = json!(self.clock.millis());
        self.store
            .exec("INSERT INTO events(body) VALUES (?)", [v.to_string()])?;
        self.events.push(v);
        Ok(())
    }
    pub fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }
    pub fn take_deliveries(&mut self) -> Vec<Delivery> {
        std::mem::take(&mut self.deliveries)
    }
    pub fn history(&self, limit: i64) -> Result<Vec<Value>> {
        self.store.query("SELECT body FROM (SELECT id,body FROM events ORDER BY id DESC LIMIT ?) ORDER BY id",[limit])?.iter()
        .map(|r|Ok(serde_json::from_str(&r.get::<String>(0)?)?)).collect()
    }
    pub fn traffic(&self, search: &str) -> Result<Vec<TrafficRow>> {
        let pattern = (!search.is_empty())
            .then(|| TrafficSearch::new(search))
            .transpose()?;
        self.store.query_filter_map(
            include_str!("traffic.sql"),
            [i64::from(pattern.is_some())],
            |r| {
                let outgoing = r.get::<String>(3)? == "out";
                let delivery = r.get::<String>(5)?;
                let findings = r.get::<i64>(9)?;
                let (status, attention) = match delivery.as_str() {
                    "failed" => ("Delivery failed".into(), true),
                    "interrupted" => ("Interrupted".into(), true),
                    "pending" => ("Pending".into(), false),
                    _ if findings > 0 => (
                        format!(
                            "Rejected · {findings} finding{}",
                            if findings == 1 { "" } else { "s" }
                        ),
                        true,
                    ),
                    _ if r.get::<i64>(12)? != 0 || r.get::<i64>(13)? != 0 => {
                        ("Rejected".into(), true)
                    }
                    _ if r.get::<i64>(11)? != 0 => ("Action failed".into(), true),
                    _ if r.get::<i64>(10)? != 0 => ("No rule matched".into(), true),
                    _ if outgoing && r.get::<i64>(14)? != 0 => ("Acknowledged".into(), false),
                    _ if outgoing && r.get::<i64>(15)? != 0 => {
                        ("Awaiting acknowledgement".into(), false)
                    }
                    _ if outgoing => ("Delivered".into(), false),
                    _ => ("Accepted".into(), false),
                };
                let row = TrafficRow {
                    id: r.get(0)?,
                    root: r.get(1)?,
                    time: r.get(2)?,
                    outgoing,
                    partner: r.get(4)?,
                    kind: r.get(6)?,
                    reference: r.get(7)?,
                    version: r.get(8)?,
                    status,
                    attention,
                };
                if let Some(query) = &pattern
                    && !row.matches(query)
                    && !query.0.is_match(&edi::latin1(&r.get::<Vec<u8>>(16)?))
                {
                    return Ok(None);
                }
                Ok(Some(row))
            },
        )
    }
    pub fn replay(&mut self, id: i64) -> Result<()> {
        let rows = self.store.query(
            "SELECT partner,bytes,metadata FROM transfers WHERE id=? AND direction='in'",
            [id],
        )?;
        let row = rows.first().context("inbound transfer not found")?;
        let p = self.partner(&row.get::<String>(0)?)?;
        let bytes = row.get::<Vec<u8>>(1)?;
        let mut metadata: Value = serde_json::from_str(&row.get::<String>(2)?)?;
        metadata["replay_of"] = json!(id);
        let replay = self
            .receive(p, &bytes, metadata, None)?
            .context("replay not recorded")?;
        self.process(p, replay, interchange::parse(&bytes))
    }
    pub fn detail(&self, id: i64) -> Result<Value> {
        let rows = self.store.query(
            "SELECT bytes,metadata,status,root FROM transfers WHERE id=?",
            [id],
        )?;
        let r = rows.first().context("unknown transfer")?;
        let bytes = r.get::<Vec<u8>>(0)?;
        let root = r.get::<i64>(3)?;
        let events = self
            .store
            .query(
                "SELECT body FROM events WHERE json_extract(body,'$.root')=?1
                 OR (json_extract(body,'$.root') IS NULL AND json_extract(body,'$.execution') IN
                     (SELECT id FROM executions WHERE root=?1)) ORDER BY id",
                [root],
            )?
            .iter()
            .map(|r| Ok(serde_json::from_str::<Value>(&r.get::<String>(0)?)?))
            .collect::<Result<Vec<_>>>()?;
        let summaries = self
            .store
            .query(
                "SELECT id, reference, ack_status FROM interchanges WHERE transfer=? ORDER BY id",
                [id],
            )?
            .iter()
            .map(|row| {
                Ok(
                    json!({"id":row.get::<i64>(0)?, "reference":row.get::<String>(1)?,
            "acknowledgement":row.get::<String>(2)?}),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        // X12 functional groups, by GS06, with their transaction sets.
        let groups = self
            .store
            .query(
                "SELECT m.grp, group_concat(m.type || ' ' || m.reference, ', ') FROM messages m
                 JOIN interchanges i ON i.id=m.interchange WHERE i.transfer=? AND m.grp<>''
                 GROUP BY m.grp ORDER BY min(m.id)",
                [id],
            )?
            .iter()
            .map(|r| Ok(json!({"reference":r.get::<String>(0)?,"sets":r.get::<String>(1)?})))
            .collect::<Result<Vec<_>>>()?;
        let findings=self.store.query("SELECT severity,code,offset,text FROM findings WHERE transfer=? ORDER BY offset,id",[id])?
            .iter().map(|r|Ok(json!({"severity":r.get::<String>(0)?,"code":r.get::<String>(1)?,"offset":r.get::<i64>(2)?,"text":r.get::<String>(3)?}))).collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"transfer":id,"segments":interchange::lines(&bytes),"findings":findings,"interchanges":summaries,"groups":groups,"status":r.get::<String>(2)?,"metadata":r.get::<String>(1)?,"bytes":bytes,"raw":edi::latin1(&bytes),"parsed":(interchange::detect(&bytes) != Some(Standard::X12)).then(|| edi::parse(&bytes)),"events":events}),
        )
    }
    pub fn partner(&self, name: &str) -> Result<usize> {
        self.workspace
            .partners
            .iter()
            .position(|p| p.name == name)
            .context("unknown partner")
    }
    // Called inside the same transaction as the raw transfer insert.
    fn allocate_transfer(store: &Store) -> Result<i64> {
        store.exec(
            "UPDATE transfer_sequence SET last_id=last_id+1 WHERE singleton=1",
            (),
        )?;
        Ok(store.query(
            "SELECT last_id FROM transfer_sequence WHERE singleton=1",
            (),
        )?[0]
            .get(0)?)
    }
    pub fn receive(
        &mut self,
        p: usize,
        raw: &[u8],
        meta: Value,
        claim: Option<&str>,
    ) -> Result<Option<i64>> {
        if let Some(c) = claim
            && !self
                .store
                .query("SELECT id FROM transfers WHERE claim=?", [c])?
                .is_empty()
        {
            return Ok(None);
        }
        let partner = &self.workspace.partners[p].name;
        let id=self.store.transaction(|s|{
            let id=s.exec("INSERT INTO transfers(id,time,direction,partner,metadata,bytes,status,claim) VALUES (?,?,'in',?,?,?,'received',?)",
                (Self::allocate_transfer(s)?,self.clock.millis(),partner.as_str(),meta.to_string(),raw.to_vec(),claim.map(str::to_owned)))?;
            s.exec("UPDATE transfers SET root=? WHERE id=?",(id,id))?; Ok(id)
        })?;
        self.event(
            "received",
            Some(id),
            json!({"transfer":id,"partner":partner,"metadata":meta}),
        )?;
        Ok(Some(id))
    }
    pub fn receipt(&self, p: usize, parsed: &interchange::Parsed) -> Receipt {
        for m in parsed.interchanges.iter().flat_map(|i| i.messages()) {
            if let Some(rule) = self.workspace.partners[p]
                .scenario()
                .iter()
                .find(|r| r.matches(&m))
            {
                for a in &rule.actions {
                    match a {
                        Action::Fault {
                            fault: Fault::Http500,
                        } => return Receipt::Error,
                        Action::Fault {
                            fault: Fault::SlowResponse,
                        } => return Receipt::Slow,
                        Action::Fault {
                            fault: Fault::NoResponse,
                        } => return Receipt::Drop,
                        _ => {}
                    }
                }
            }
        }
        Receipt::Accepted
    }
    fn findings(&mut self, id: i64, interchange: Option<i64>, items: &[Finding]) -> Result<()> {
        for f in items {
            self.store.exec("INSERT INTO findings(transfer,interchange,severity,code,offset,text) VALUES (?,?,'error',?,?,?)",(id,interchange,f.code,f.offset as i64,f.text.as_str()))?;
            self.event("finding", Some(id), json!({"transfer":id,"finding":f}))?;
        }
        Ok(())
    }
    fn summary(s: &Store, transfer: i64, i: &Interchange) -> Result<(i64, Vec<i64>)> {
        let first = |v: Vec<String>| v.into_iter().next().unwrap_or_default();
        let id=s.exec("INSERT INTO interchanges(transfer,sender,recipient,reference,test_flag,ack_requested,ack_status) VALUES (?,?,?,?,?,?,'unknown')",
        (transfer,first(i.sender()),first(i.recipient()),i.reference(),i.test_flag(),if i.ack_requested() {"1"} else {""}))?;
        let mut messages = Vec::new();
        for m in i.messages() {
            messages.push(s.exec("INSERT INTO messages(interchange,type,version,release,reference,segment_count,grp) VALUES (?,?,?,?,?,?,?)",
            (id,m.kind.as_str(),m.directory.as_str(),m.release.as_str(),m.reference.as_str(),m.segments as i64,m.group.as_str()))?);
        }
        Ok((id, messages))
    }
    fn task(
        &mut self,
        p: usize,
        root: Option<i64>,
        trigger: (Option<i64>, Option<i64>),
        name: &str,
        actions: Vec<Action>,
        i: Option<Arc<Interchange>>,
    ) -> Result<Task> {
        let execution = self.store.exec(
            "INSERT INTO executions(root,interchange,message,partner,name) VALUES (?,?,?,?,?)",
            (
                root,
                trigger.0,
                trigger.1,
                self.workspace.partners[p].name.as_str(),
                name,
            ),
        )?;
        let case = self.workspace.partners[p].case_id().map(str::to_owned);
        self.event(
            "execution",
            root,
            json!({"execution":execution,"name":name,"case":case,"interchange":trigger.0,"message":trigger.1}),
        )?;
        Ok(Task {
            execution,
            root,
            partner: p,
            inbound: i,
            actions: actions.into(),
            faults: Vec::new(),
        })
    }
    pub fn process(&mut self, p: usize, id: i64, parsed: interchange::Parsed) -> Result<()> {
        self.findings(id, None, &parsed.findings)?;
        for mut i in parsed.interchanges {
            let partner = &self.workspace.partners[p];
            let offset = i.offset();
            if i.standard() != partner.standard {
                let text = format!(
                    "{} interchange for the {} partner {}",
                    i.standard().label(),
                    partner.standard.label(),
                    partner.name
                );
                i.findings_mut()
                    .push(Finding::new("standard", offset, text));
            }
            let qualified = |expected: &[String], found: Vec<String>| match i.standard() {
                Standard::Edifact => found == expected,
                // ISA always carries a qualifier; an identity without one means ZZ.
                Standard::X12 => {
                    found.first() == expected.first()
                        && found.get(1).map(String::as_str)
                            == Some(expected.get(1).map_or("ZZ", String::as_str))
                }
            };
            let identities_match = qualified(&self.workspace.identity, i.sender())
                && qualified(&partner.identity, i.recipient());
            if !identities_match {
                i.findings_mut().push(Finding::new(
                    "identity",
                    offset,
                    "interchange identities do not match receive channel",
                ));
            }
            let charset_differs = i.charset().is_some_and(|c| c != partner.charset);
            if charset_differs {
                i.findings_mut()
                    .push(Finding::new("charset", offset, "unexpected character set"));
            }
            let (iid, mids) = self.store.transaction(|s| Self::summary(s, id, &i))?;
            let messages = i.messages();
            let rules: Vec<_> = messages
                .iter()
                .map(|m| partner.scenario().iter().find(|r| r.matches(m)).cloned())
                .collect();
            let faults: Vec<Fault> = rules
                .iter()
                .flatten()
                .flat_map(|r| &r.actions)
                .filter_map(|a| {
                    if let Action::Fault { fault } = a {
                        Some(*fault)
                    } else {
                        None
                    }
                })
                .collect();
            let ack = partner.policy() == Policy::Always
                || (partner.policy() == Policy::Requested && i.ack_requested());
            let accepted = i.accepted() && !faults.contains(&Fault::Reject);
            let wrong_standard = i.standard() != partner.standard;
            self.findings(id, Some(iid), i.findings())?;
            self.event("validated",Some(id),json!({"interchange":iid,"accepted":accepted,"messages":mids.len(),"findings":i.findings().len()}))?;
            let i = Arc::new(i);
            if ack
                && !wrong_standard
                && !faults.contains(&Fault::SuppressAck)
                && !i.carries_acknowledgement()
                && !i.reference().is_empty()
            {
                let mut t = self.task(
                    p,
                    Some(id),
                    (Some(iid), None),
                    "acknowledge",
                    vec![],
                    Some(i.clone()),
                )?;
                for m in interchange::acknowledge(&self.workspace.partners[p], &i, accepted) {
                    self.send(&mut t, &m)?;
                }
            }
            if !accepted {
                continue;
            }
            for ack in i.acknowledgements() {
                self.correlate(p, id, &ack)?;
            }
            for ((m, mid), rule) in messages.iter().zip(mids).zip(rules) {
                if m.rejected {
                    self.event(
                        "rejected_message",
                        Some(id),
                        json!({"message":mid,"type":m.kind}),
                    )?;
                    continue;
                }
                if let Some(rule) = rule {
                    let t = self.task(
                        p,
                        Some(id),
                        (Some(iid), Some(mid)),
                        &rule.name,
                        rule.actions,
                        Some(i.clone()),
                    )?;
                    self.execute(t)?;
                } else {
                    self.event("no_match", Some(id), json!({"message":mid,"type":m.kind}))?;
                }
            }
        }
        Ok(())
    }
    fn correlate(&mut self, p: usize, root: i64, ack: &AckInfo) -> Result<()> {
        let partner = &self.workspace.partners[p];
        let sql = if ack.numeric {
            "SELECT i.id FROM interchanges i JOIN transfers t ON t.id=i.transfer WHERE t.direction='out' AND t.partner=? AND CAST(i.reference AS INTEGER)=CAST(? AS INTEGER)"
        } else {
            "SELECT i.id FROM interchanges i JOIN transfers t ON t.id=i.transfer WHERE t.direction='out' AND t.partner=? AND i.reference=?"
        };
        let rows = self
            .store
            .query(sql, (partner.name.as_str(), ack.reference.as_str()))?;
        let parties_match = ack
            .parties
            .as_ref()
            .is_none_or(|(from, to)| *from == partner.identity && *to == self.workspace.identity);
        if rows.len() == 1 && parties_match {
            let iid = rows[0].get::<i64>(0)?;
            self.store.exec(
                "UPDATE interchanges SET ack_status=? WHERE id=?",
                (ack.status, iid),
            )?;
            self.event(
                "acknowledged",
                Some(root),
                json!({"interchange":iid,"status":ack.status}),
            )?;
        } else {
            self.event(
                "unresolved_ack",
                Some(root),
                json!({"reference":ack.reference,"candidates":rows.len()}),
            )?;
        }
        Ok(())
    }
    fn send(&mut self, t: &mut Task, m: &Outgoing) -> Result<()> {
        let partner = &self.workspace.partners[t.partner];
        let name = partner.name.as_str();
        let now = self.clock.millis();
        let (id,bytes,root)=self.store.transaction(|s|{
            let last=s.query("SELECT reference FROM partner_state WHERE partner=?",[name])?[0].get::<i64>(0)?;
            let next=if t.faults.contains(&Fault::DuplicateReference) && last>0 {last} else {last.checked_add(1).context("reference counter exhausted")?};
            let x = partner.x12();
            let applications = (
                x.application.as_deref().unwrap_or(&partner.identity[0]),
                x.workspace_application.as_deref().unwrap_or(&self.workspace.identity[0]),
            );
            let route = interchange::Route { sender: &partner.identity, recipient: &self.workspace.identity, applications };
            let mut bytes=interchange::envelope(partner,&route,&next.to_string(),
                DateTime::from_timestamp_millis(now).context("invalid clock")?,t.faults.contains(&Fault::WrongCount),m)?;
            if t.faults.contains(&Fault::Malformed){bytes.pop();}
            s.exec("UPDATE partner_state SET reference=? WHERE partner=?",(next,name))?;
            let metadata = partner.send.metadata();
            let id=s.exec("INSERT INTO transfers(id,root,time,direction,partner,metadata,bytes,status,execution) VALUES (?,?,?,'out',?,?,?,'pending',?)",
                (Self::allocate_transfer(s)?,t.root,now,name,metadata.to_string(),bytes.clone(),t.execution))?;
            let root=t.root.unwrap_or(id);
            if t.root.is_none(){s.exec("UPDATE transfers SET root=? WHERE id=?",(root,id))?;s.exec("UPDATE executions SET root=? WHERE id=?",(root,t.execution))?;}
            if let Some(i)=interchange::parse(&bytes).interchanges.first(){Self::summary(s,id,i)?;}
            Ok((id,bytes,root))
        })?;
        t.root = Some(root);
        self.deliveries.push(Delivery {
            id,
            bytes,
            channel: partner.send.clone(),
        });
        self.event(
            "outbound",
            Some(root),
            json!({"transfer":id,"execution":t.execution,"type":m.kind(),"partner":name}),
        )?;
        if t.faults.contains(&Fault::Duplicate) {
            self.repeat_delivery(id, Some(t.execution))?;
        }
        Ok(())
    }
    pub fn finish_delivery(&mut self, id: i64, result: Result<String, String>) -> Result<()> {
        let rows = self
            .store
            .query("SELECT root FROM transfers WHERE id=?", [id])?;
        let root = rows.first().context("unknown transfer")?.get::<i64>(0)?;
        let status = if result.is_ok() {
            "delivered"
        } else {
            "failed"
        };
        self.store
            .exec("UPDATE transfers SET status=? WHERE id=?", (status, id))?;
        self.event(
            status,
            Some(root),
            json!({"transfer":id,"detail":result.unwrap_or_else(|e|e)}),
        )
    }
    fn execute(&mut self, mut t: Task) -> Result<()> {
        while let Some(action) = t.actions.pop_front() {
            let desc = format!("{action:?}");
            if let Action::Delay { delay_ms } = action {
                let due = self.clock.millis() + i64::from(delay_ms);
                self.sequence += 1;
                self.event(
                    "scheduled",
                    t.root,
                    json!({"execution":t.execution,"due":due}),
                )?;
                self.pending.insert((due, self.sequence), t);
                return Ok(());
            }
            let result = self.action(&mut t, &action);
            let script = match &action {
                Action::Script { script, .. } => Some(script),
                _ => None,
            };
            self.event(
                "action",
                t.root,
                json!({
                    "execution":t.execution,"action":desc,"script":script,
                    "error":result.as_ref().err().map(|e|format!("{e:#}"))
                }),
            )?;
            if result.is_err() {
                break;
            }
        }
        Ok(())
    }
    fn action(&mut self, t: &mut Task, action: &Action) -> Result<()> {
        match action {
            Action::Fault { fault } => t.faults.push(*fault),
            Action::Delay { .. } => unreachable!(),
            Action::Document { document } => {
                let standard = self.workspace.partners[t.partner].standard;
                let m = interchange::document(
                    standard,
                    &fs::read(self.workspace.root.join(document))?,
                )?;
                self.send(t, &m)?;
            }
            Action::Script {
                script: path,
                function,
            } => {
                let p = &self.workspace.partners[t.partner].name;
                let rows = self.store.query(
                    "SELECT reference,data FROM partner_state WHERE partner=?",
                    [p.as_str()],
                )?;
                let reference = rows[0].get::<i64>(0)?;
                let state: BTreeMap<_, _> = serde_json::from_str(&rows[0].get::<String>(1)?)?;
                let standard = self.workspace.partners[t.partner].standard;
                let view = t.inbound.as_deref().map_or(Value::Null, Interchange::view);
                let effects = script::run(
                    &script::read_source(self.workspace.root.join(path))?,
                    function,
                    &script::Input {
                        interchange: &view,
                        legacy: t.inbound.as_deref().and_then(Interchange::legacy),
                        state: &state,
                        reference,
                        now: self.clock.millis(),
                    },
                )?;
                let messages = effects
                    .sends
                    .iter()
                    .enumerate()
                    .map(|(n, send)| {
                        match send {
                            Value::String(raw) if standard == Standard::Edifact => {
                                edi::script_document(raw).map(Outgoing::Edifact)
                            }
                            Value::String(_) => {
                                anyhow::bail!("send a message from api.build to an X12 partner")
                            }
                            other => contract::built(other, standard),
                        }
                        .with_context(|| format!("script send {}", n + 1))
                    })
                    .collect::<Result<Vec<_>>>()?;
                ensure!(
                    !effects.acknowledge || t.inbound.is_some(),
                    "an acknowledgement needs an inbound interchange"
                );
                self.store.exec(
                    "UPDATE partner_state SET reference=?,data=? WHERE partner=?",
                    (
                        effects.reference,
                        serde_json::to_string(&effects.state)?,
                        p.as_str(),
                    ),
                )?;
                for line in effects.logs {
                    self.event(
                        "script_log",
                        t.root,
                        json!({"execution":t.execution,"text":line}),
                    )?;
                }
                for feature in effects.deprecated {
                    self.event(
                        "deprecated",
                        t.root,
                        json!({"execution":t.execution,"script":path,"feature":feature}),
                    )?;
                }
                if effects.acknowledge {
                    let inbound = t.inbound.clone().unwrap();
                    for m in interchange::acknowledge(
                        &self.workspace.partners[t.partner],
                        &inbound,
                        true,
                    ) {
                        self.send(t, &m)?;
                    }
                }
                for m in messages {
                    self.send(t, &m)?;
                }
            }
        }
        Ok(())
    }
    pub fn next_wait(&self) -> Option<Duration> {
        self.pending
            .first_key_value()
            .map(|((ms, _), _)| Duration::from_millis((*ms - self.clock.millis()).max(0) as u64))
    }
    pub fn tick(&mut self) -> Result<()> {
        while self.next_wait() == Some(Duration::ZERO) {
            let (_, task) = self.pending.pop_first().unwrap();
            self.execute(task)?;
        }
        Ok(())
    }
    pub fn initiate(&mut self, name: &str, file: &Path, function: Option<&str>) -> Result<()> {
        let p = self.partner(name)?;
        let action = match function {
            Some(f) => Action::Script {
                script: file.into(),
                function: f.into(),
            },
            None => Action::Document {
                document: file.into(),
            },
        };
        let t = self.task(p, None, (None, None), "command: send", vec![action], None)?;
        self.execute(t)
    }
    /// Switches a partner to an installed case and records the choice in
    /// `vendredi.toml`.
    pub fn use_case(&mut self, name: &str, id: &str) -> Result<()> {
        let p = self.partner(name)?;
        let mut partner = self.workspace.partners[p].clone();
        let from = partner.case_id().map(str::to_owned);
        partner.activate(&self.workspace.cases, id)?;
        // Re-read the files so edits since the workspace opened are checked.
        crate::case::Bundle::load(&self.workspace.root.join(crate::case::DIRECTORY).join(id))?;
        crate::config::persist_case(&self.workspace.root, name, id)?;
        self.workspace.partners[p] = partner;
        self.event(
            "case_switched",
            None,
            json!({"partner":name,"from":from,"to":id}),
        )
    }
    /// Receives a sample message of the partner's active case as if the
    /// integration had sent it, enveloped from the workspace to the partner.
    pub fn sample(&mut self, name: &str, sample: &str) -> Result<i64> {
        let p = self.partner(name)?;
        let partner = &self.workspace.partners[p];
        let id = partner
            .case_id()
            .with_context(|| format!("partner {name} has no active case"))?
            .to_owned();
        let case = self
            .workspace
            .cases
            .iter()
            .find(|c| c.id == id)
            .with_context(|| format!("partner {name} has no active case with samples"))?;
        let document = &case
            .sample(sample)
            .with_context(|| format!("case {id} has no sample named {sample}"))?
            .document;
        let path = self
            .workspace
            .root
            .join(crate::case::DIRECTORY)
            .join(&id)
            .join(document);
        let message = interchange::document(
            partner.standard,
            &fs::read(&path).with_context(|| path.display().to_string())?,
        )?;
        let next = self.store.query(
            "SELECT last_id+1 FROM transfer_sequence WHERE singleton=1",
            (),
        )?[0]
            .get::<i64>(0)?;
        let x = partner.x12();
        let applications = (
            x.workspace_application
                .as_deref()
                .unwrap_or(&self.workspace.identity[0]),
            x.application.as_deref().unwrap_or(&partner.identity[0]),
        );
        let route = interchange::Route {
            sender: &self.workspace.identity,
            recipient: &partner.identity,
            applications,
        };
        let reference = match partner.standard {
            Standard::Edifact => format!("S{next}"),
            Standard::X12 => next.to_string(),
        };
        let time = DateTime::from_timestamp_millis(self.clock.millis()).context("invalid clock")?;
        let bytes = interchange::envelope(partner, &route, &reference, time, false, &message)?;
        let transfer = self
            .receive(p, &bytes, json!({"case":id,"sample":sample}), None)?
            .context("sample not recorded")?;
        self.process(p, transfer, interchange::parse(&bytes))?;
        Ok(transfer)
    }
    /// Runs a partner-initiated send defined by the active case.
    pub fn initiate_case(&mut self, name: &str, send: &str) -> Result<()> {
        let p = self.partner(name)?;
        let partner = &self.workspace.partners[p];
        let i = partner
            .active
            .as_ref()
            .and_then(|a| a.initiate.iter().find(|i| i.name == send))
            .with_context(|| {
                format!(
                    "case {} has no partner-initiated send named {send}",
                    partner.case_id().unwrap_or("(none)")
                )
            })?
            .clone();
        let t = self.task(
            p,
            None,
            (None, None),
            &format!("initiate: {}", i.name),
            i.actions,
            None,
        )?;
        self.execute(t)
    }
    pub fn resend(&mut self, id: i64) -> Result<()> {
        self.repeat_delivery(id, None)
    }
    fn repeat_delivery(&mut self, id: i64, parent_execution: Option<i64>) -> Result<()> {
        let rows = self.store.query(
            "SELECT bytes,partner,root FROM transfers WHERE id=? AND direction='out'",
            [id],
        )?;
        let r = rows
            .first()
            .ok_or_else(|| anyhow!("outbound transfer {id} not found"))?;
        let name = r.get::<String>(1)?;
        let root = r.get::<i64>(2)?;
        let bytes = r.get::<Vec<u8>>(0)?;
        let p = self.partner(&name)?;
        let execution = match parent_execution {
            Some(id) => id,
            None => {
                self.task(p, Some(root), (None, None), "command: resend", vec![], None)?
                    .execution
            }
        };
        let metadata = self.workspace.partners[p].send.metadata();
        let new=self.store.transaction(|s|{
            let new=s.exec("INSERT INTO transfers(id,root,time,direction,partner,metadata,bytes,status,execution,resend_of) VALUES (?,?,?,'out',?,?,?,'pending',?,?)",
            (Self::allocate_transfer(s)?,root,self.clock.millis(),name.as_str(),metadata.to_string(),bytes.clone(),execution,id))?;
            Ok(new)
        })?;
        self.deliveries.push(Delivery {
            id: new,
            channel: self.workspace.partners[p].send.clone(),
            bytes,
        });
        self.event(
            "resent",
            Some(root),
            json!({"transfer":new,"resend_of":id,"execution":execution,"partner":name}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::x12;
    use std::sync::atomic::{AtomicI64, Ordering};
    struct Manual(AtomicI64);
    impl Clock for Manual {
        fn millis(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    const ORDER: &[u8] = include_bytes!("../../../examples/orders/order.edi");
    fn fixture() -> Result<(tempfile::TempDir, Workspace)> {
        let dir = tempfile::tempdir()?;
        crate::config::orders_example(dir.path())?;
        let w = Workspace::load(dir.path())?;
        Ok((dir, w))
    }
    /// The active case's rules, which tests rewrite in place.
    fn rules(p: &mut crate::config::Partner) -> &mut Vec<crate::config::Rule> {
        &mut p.active.as_mut().expect("active case").rules
    }
    fn input(e: &mut Engine, p: usize, bytes: &[u8]) -> Result<i64> {
        let id = e.receive(p, bytes, json!({}), None)?.unwrap();
        e.process(p, id, interchange::parse(bytes))?;
        Ok(id)
    }
    #[test]
    fn traffic_inspection_search_and_replay() -> Result<()> {
        let (_dir, w) = fixture()?;
        let mut engine = Engine::open(w, Arc::new(SystemClock))?;
        let id = input(&mut engine, 0, ORDER)?;
        assert_eq!(engine.traffic("po42")?[0].id, id);
        let orders = engine.traffic("orders")?;
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].status, "Accepted");
        let original = engine.take_deliveries().pop().unwrap().id;
        engine.resend(original)?;
        engine.resend(engine.traffic("ORDRSP")?[0].id)?;
        assert_eq!(engine.traffic("ORDRSP")?.len(), 3);
        let invalid = input(&mut engine, 0, b"garbage")?;
        assert!(
            engine
                .traffic("")?
                .iter()
                .any(|t| t.id == invalid && t.attention)
        );
        assert_eq!(
            engine.detail(invalid)?["findings"][0]["code"],
            "unterminated"
        );
        engine.replay(id)?;
        let replay = engine.traffic("po42")?[0].id;
        assert_ne!(id, replay);
        let detail = engine.detail(replay)?;
        assert_eq!(
            serde_json::from_str::<Value>(detail["metadata"].as_str().unwrap())?["replay_of"],
            id
        );
        assert_eq!(detail["segments"][2]["elements"][1][0], "PO42");
        Ok(())
    }
    #[test]
    fn search_visible_fields_and_latin1_contents() -> Result<()> {
        let (_dir, mut workspace) = fixture()?;
        workspace.partners[0].name = "Café Nord".into();
        let clock = Arc::new(Manual(AtomicI64::new(10_000)));
        let mut engine = Engine::open(workspace, clock)?;
        let inbound = input(&mut engine, 0, ORDER)?;
        for delivery in engine.take_deliveries() {
            engine.finish_delivery(delivery.id, Ok(String::new()))?;
        }
        for term in [
            "CAFÉ NORD",
            "in",
            "accepted",
            "orders",
            "ORDER42",
            &local_time(10_000),
        ] {
            assert!(
                engine.traffic(term)?.iter().any(|r| r.id == inbound),
                "{term}"
            );
        }
        for term in ["out", "delivered"] {
            let results = engine.traffic(term)?;
            assert!(!results.is_empty() && results.iter().all(|r| r.outgoing));
        }
        let invalid = input(&mut engine, 0, b"pi\xe8ce [A]*")?;
        for term in ["PIÈCE", "[a]*", "⚠ Rejected · 1 finding"] {
            assert_eq!(
                engine
                    .traffic(term)?
                    .iter()
                    .map(|r| r.id)
                    .collect::<Vec<_>>(),
                [invalid]
            );
        }
        assert!(engine.traffic("not present")?.is_empty());
        Ok(())
    }
    #[test]
    fn transports_share_fixture_bytes_and_decisions() -> Result<()> {
        let date = "2026-11-24T10:30:00Z".parse::<DateTime<Utc>>()?;
        let mut traces = Vec::new();
        for folder in [false, true] {
            let (_dir, mut w) = fixture()?;
            let metadata = if folder {
                json!({"file":"order.edi"})
            } else {
                json!({"path":"/supplier/orders"})
            };
            if folder {
                w.partners[0].send = SendChannel::Folder {
                    directory: "out".into(),
                };
            }
            let mut engine =
                Engine::open(w, Arc::new(Manual(AtomicI64::new(date.timestamp_millis()))))?;
            let id = engine.receive(0, ORDER, metadata, None)?.unwrap();
            engine.process(0, id, interchange::parse(ORDER))?;
            let replies = engine.take_deliveries();
            assert_eq!(replies.len(), 2);
            for (d, expected) in replies.iter().zip([
                include_str!("../../../examples/orders/expected/ack.edi"),
                include_str!("../../../examples/orders/expected/response.edi"),
            ]) {
                assert_eq!(d.bytes, expected.trim_end().as_bytes());
            }
            let mut trace = engine.take_events();
            trace[0].as_object_mut().unwrap().remove("metadata");
            traces.push(trace);
        }
        assert_eq!(traces[0], traces[1]);
        Ok(())
    }
    #[test]
    fn retention_preserves_references_and_unique_delivery_filenames() -> Result<()> {
        let (_dir, mut w) = fixture()?;
        w.retention_days = 1;
        let clock = Arc::new(Manual(AtomicI64::new(1000)));
        let mut e = Engine::open(w.clone(), clock.clone())?;
        input(&mut e, 0, ORDER)?;
        let previous = e.take_deliveries().pop().unwrap().id;
        drop(e);
        clock.0.store(1000 + 2 * 86_400_000, Ordering::SeqCst);
        let mut e = Engine::open(w, clock)?;
        assert!(e.detail(previous).is_err());
        e.initiate("Supplier", Path::new("confirmation.edi"), None)?;
        let transfer = e.take_deliveries().pop().unwrap();
        assert!(transfer.id > previous);
        assert_eq!(
            edi::parse(&transfer.bytes).interchanges[0].header.at(4, 0),
            "3"
        );
        assert!(
            e.detail(transfer.id)?["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["name"] == "command: send")
        );
        Ok(())
    }
    #[test]
    fn clear_history_uses_latest_transfer_and_skips_active_exchanges() -> Result<()> {
        let (_dir, mut w) = fixture()?;
        let mut other = w.partners[0].clone();
        other.name = "Other".into();
        w.partners.push(other);
        let clock = Arc::new(Manual(AtomicI64::new(0)));
        let mut e = Engine::open(w, clock.clone())?;
        let old = input(&mut e, 0, ORDER)?;
        let spanning = input(&mut e, 0, ORDER)?;
        let replies = e.take_deliveries();
        for d in &replies {
            e.finish_delivery(d.id, Ok(String::new()))?;
        }
        clock.0.store(100_000, Ordering::SeqCst);
        e.resend(replies.last().unwrap().id)?;
        let resend = e.take_deliveries().pop().unwrap().id;
        e.finish_delivery(resend, Ok(String::new()))?;
        let invalid = input(&mut e, 0, b"bad")?;
        let other = input(&mut e, 1, b"bad")?;
        let in_flight = input(&mut e, 0, ORDER)?;
        assert!(!e.take_deliveries().is_empty()); // Leave this exchange awaiting delivery.
        rules(&mut e.workspace.partners[0])[0].actions = vec![Action::Delay { delay_ms: 180_000 }];
        let delayed = input(&mut e, 0, ORDER)?;
        for d in e.take_deliveries() {
            e.finish_delivery(d.id, Ok(String::new()))?;
        }
        clock.0.store(160_000, Ordering::SeqCst);
        assert_eq!(e.clear_history(HistoryPeriod::Minute, Some("Supplier"))?, 2);
        for id in [spanning, resend, invalid] {
            assert!(e.detail(id).is_err());
        }
        for id in [old, other, in_flight, delayed] {
            assert!(e.detail(id).is_ok());
        }
        assert!(
            e.history(500)?
                .iter()
                .all(|e| e["root"] != spanning && e["root"] != invalid)
        );
        assert_eq!(e.clear_history(HistoryPeriod::Minute, Some("Supplier"))?, 0);
        assert_eq!(e.clear_history(HistoryPeriod::Week, None)?, 2);
        assert!(e.detail(delayed).is_ok());
        assert!(e.detail(in_flight).is_ok());
        Ok(())
    }

    #[test]
    fn clear_history_ranges_include_boundary_and_removed_partner() -> Result<()> {
        for (period, span) in [
            (HistoryPeriod::Minute, 60_000),
            (HistoryPeriod::Hour, 3_600_000),
            (HistoryPeriod::Day, 86_400_000),
            (HistoryPeriod::Week, 604_800_000),
        ] {
            let (_dir, w) = fixture()?;
            let clock = Arc::new(Manual(AtomicI64::new(0)));
            let mut engine = Engine::open(w, clock.clone())?;
            let older = input(&mut engine, 0, b"old")?;
            clock.0.store(1, Ordering::SeqCst);
            let boundary = input(&mut engine, 0, b"boundary")?;
            clock.0.store(span + 2, Ordering::SeqCst);
            let future = input(&mut engine, 0, b"future")?;
            engine.workspace.partners[0].name = "Replacement".into();
            clock.0.store(span + 1, Ordering::SeqCst);
            assert_eq!(engine.clear_history(period, Some("Supplier"))?, 1);
            assert!(engine.detail(boundary).is_err());
            assert!(engine.detail(older).is_ok());
            assert!(engine.detail(future).is_ok());
        }
        Ok(())
    }

    #[test]
    fn partner_delay_script_state_and_faults_are_isolated() -> Result<()> {
        let (dir, mut w) = fixture()?;
        fs::write(
            dir.path().join("handle.js"),
            "function run(i,a){a.set('calls',(a.get('calls')||0)+1);a.log(a.now());a.send(a.build('ORDRSP:D:96A:UN', \"BGM+231'\"))}",
        )?;
        let rule = &mut rules(&mut w.partners[0])[0];
        rule.actions = vec![
            Action::Delay { delay_ms: 20 },
            Action::Script {
                script: "handle.js".into(),
                function: "run".into(),
            },
        ];
        let mut other = w.partners[0].clone();
        other.name = "Other".into();
        rules(&mut other)[0].actions = vec![
            Action::Fault {
                fault: Fault::SuppressAck,
            },
            Action::Fault {
                fault: Fault::WrongCount,
            },
            Action::Document {
                document: "confirmation.edi".into(),
            },
        ];
        w.partners.push(other);
        let clock = Arc::new(Manual(AtomicI64::new(1000)));
        let mut e = Engine::open(w.clone(), clock.clone())?;
        input(&mut e, 0, ORDER)?;
        input(&mut e, 1, ORDER)?;
        let d = e.take_deliveries();
        assert_eq!(d.len(), 2);
        let a = edi::parse(&d[0].bytes);
        let b = edi::parse(&d[1].bytes);
        assert_eq!(
            a.interchanges[0].header.at(4, 0),
            b.interchanges[0].header.at(4, 0)
        );
        assert!(!b.interchanges[0].findings.is_empty());
        clock.0.store(1020, Ordering::SeqCst);
        e.tick()?;
        let delayed = e.take_deliveries().pop().unwrap();
        assert_eq!(
            edi::parse(&delayed.bytes).interchanges[0].messages[0].kind(),
            "ORDRSP"
        );
        assert_eq!(delayed.bytes, b"UNB+UNOC:3+SUPPLIER:ZZ+BUYER:ZZ+700101:0000+2'UNH+1+ORDRSP:D:96A:UN'BGM+231'UNT+3+1'UNZ+1+2'");
        assert!(
            e.take_events()
                .iter()
                .any(|event| event["event"] == "script_log" && event["text"] == "1020")
        );
        e.finish_delivery(delayed.id, Err("offline".into()))?;
        e.resend(delayed.id)?;
        let resent = e.take_deliveries().pop().unwrap();
        assert_eq!(resent.bytes, delayed.bytes);
        e.finish_delivery(resent.id, Ok("ok".into()))?;
        let ack=b"UNB+UNOC:3+BUYER:ZZ+SUPPLIER:ZZ+261124:1100+A'UNH+1+CONTRL:D:96A:UN'UCI+1+SUPPLIER:ZZ+BUYER:ZZ+7'UNT+3+1'UNZ+1+A'";
        input(&mut e, 0, ack)?;
        assert!(e.take_deliveries().is_empty());
        let rows = e.store.query(
            "SELECT ack_status FROM interchanges WHERE reference='1' ORDER BY id",
            (),
        )?;
        assert_eq!(rows[0].get::<String>(0)?, "accepted");
        assert_eq!(rows[1].get::<String>(0)?, "unknown");
        drop(e);
        let mut e = Engine::open(w, clock)?;
        assert_eq!(e.detail(delayed.id)?["status"], "failed");
        assert_eq!(e.detail(resent.id)?["status"], "delivered");
        let states = e
            .store
            .query("SELECT data FROM partner_state ORDER BY partner", ())?;
        assert_eq!(states[0].get::<String>(0)?, "{}");
        assert!(states[1].get::<String>(0)?.contains("calls"));
        e.initiate("Supplier", Path::new("confirmation.edi"), None)?;
        assert_eq!(
            edi::parse(&e.take_deliveries()[0].bytes).interchanges[0]
                .header
                .at(4, 0),
            "3"
        );
        Ok(())
    }
    fn script_replies(
        w: &Workspace,
        dir: &Path,
        source: &str,
        order: &[u8],
    ) -> Result<(Vec<Delivery>, Vec<Value>)> {
        fs::write(dir.join("reply.js"), source)?;
        let mut w = w.clone();
        w.partners[0].acknowledge = Policy::Never;
        rules(&mut w.partners[0])[0].actions = vec![Action::Script {
            script: "reply.js".into(),
            function: "reply".into(),
        }];
        fs::remove_dir_all(dir.join(".vendredi")).ok();
        let date = "2026-11-24T10:30:00Z".parse::<DateTime<Utc>>()?;
        let mut e = Engine::open(w, Arc::new(Manual(AtomicI64::new(date.timestamp_millis()))))?;
        input(&mut e, 0, order)?;
        let events = e.take_events();
        assert!(events.iter().all(|v| v["error"].is_null()), "{events:#?}");
        Ok((e.take_deliveries(), events))
    }
    fn deprecations(events: &[Value]) -> Vec<&str> {
        events
            .iter()
            .filter(|v| v["event"] == "deprecated")
            .map(|v| v["feature"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn structured_script_messages_match_legacy_text() -> Result<()> {
        let (dir, w) = fixture()?;
        let (legacy, events) = script_replies(
            &w,
            dir.path(),
            "function reply(i, api) { api.send(api.build('ORDRSP:D:96A:UN', \"BGM+231+\" + i.messages[0].segments[1].elements[1][0] + \"+9'\")); }",
            ORDER,
        )?;
        assert_eq!(
            deprecations(&events),
            [
                "interchange.messages",
                "api.build(type, text)",
                "api.send(text)"
            ]
        );
        let (structured, events) = script_replies(
            &w,
            dir.path(),
            "function reply(i, api) {
                const order = i.groups[0].messages[0];
                const bgm = order.segments.find(s => s[0] === 'BGM');
                api.send(api.build('ORDRSP', order.version, [['BGM', '231', bgm[2], '9']]));
            }",
            ORDER,
        )?;
        assert!(deprecations(&events).is_empty());
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].bytes, structured[0].bytes);
        assert_eq!(
            structured[0].bytes,
            b"UNB+UNOC:3+SUPPLIER:ZZ+BUYER:ZZ+261124:1030+1'UNH+1+ORDRSP:D:96A:UN'BGM+231+PO42+9'UNT+3+1'UNZ+1+1'"
        );
        Ok(())
    }

    #[test]
    fn script_values_round_trip_custom_separators_and_acknowledge() -> Result<()> {
        let (dir, mut w) = fixture()?;
        w.partners[0].separators = Some("*;,! ~".into());
        let order = b"UNA*;,! ~UNB;UNOC*3;BUYER*ZZ;SUPPLIER*ZZ;261124*1030;7~UNH;1;ORDERS*D*96A*UN~FTX;AAI;;;a!;b!*c!~d!!e'f+g:h?i~UNT;3;1~UNZ;1;7~";
        let (replies, _) = script_replies(
            &w,
            dir.path(),
            "function reply(i, api) {
                const ftx = i.groups[0].messages[0].segments[0];
                api.log(JSON.stringify(ftx));
                api.ack();
                api.send(api.build('ORDRSP', 'D96A', [['FTX', 'AAI', '', '', ftx[4]]]));
            }",
            order,
        )?;
        assert_eq!(replies.len(), 2);
        let parsed = edi::parse(&replies[1].bytes);
        let i = &parsed.interchanges[0];
        assert!(i.findings.is_empty(), "{:?}", i.findings);
        assert_eq!(i.syntax.0, *b"*;,! ~");
        assert_eq!(i.messages[0].segments[1].at(3, 0), "a;b*c~d!e'f+g:h?i");
        assert_eq!(
            edi::parse(&replies[0].bytes).interchanges[0].messages[0].kind(),
            "CONTRL"
        );
        Ok(())
    }

    #[test]
    fn invalid_script_messages_fail_the_action() -> Result<()> {
        let (dir, mut w) = fixture()?;
        fs::write(
            dir.path().join("reply.js"),
            "function reply(i, api) { api.send(api.build('ORDRSP', 'D96A', [['UNZ', '1']])); }",
        )?;
        rules(&mut w.partners[0])[0].actions = vec![Action::Script {
            script: "reply.js".into(),
            function: "reply".into(),
        }];
        let mut e = Engine::open(w, Arc::new(SystemClock))?;
        input(&mut e, 0, ORDER)?;
        let error = e
            .take_events()
            .into_iter()
            .find_map(|v| v["error"].as_str().map(str::to_owned))
            .unwrap();
        assert!(
            error.contains("script send 1") && error.contains("envelope"),
            "{error}"
        );
        Ok(())
    }

    fn install(dir: &Path, json: &str) -> Result<()> {
        let bundle =
            crate::case::Bundle::resolve(crate::case::Case::parse(json.as_bytes())?, |p| {
                anyhow::bail!("{} missing", p.display())
            })?;
        bundle.write(&dir.join("cases").join(&bundle.case.id))
    }

    #[test]
    fn cases_switch_persist_and_drive_samples() -> Result<()> {
        let (dir, _) = fixture()?;
        install(
            dir.path(),
            include_str!("../../../examples/cases/reject-orders.json"),
        )?;
        install(
            dir.path(),
            include_str!("../../../examples/cases/send-invoice.json"),
        )?;
        let w = Workspace::load(dir.path())?;
        assert_eq!(w.cases.len(), 3);
        let clock = Arc::new(Manual(AtomicI64::new(1000)));
        let mut e = Engine::open(w, clock.clone())?;
        e.use_case("Supplier", "reject-orders")?;
        input(&mut e, 0, ORDER)?;
        let replies = e.take_deliveries();
        assert_eq!(replies.len(), 1);
        assert!(replies[0].bytes.windows(2).any(|w| w == b"+4"));
        let events = e.take_events();
        assert!(
            events
                .iter()
                .any(|v| v["event"] == "case_switched" && v["to"] == "reject-orders")
        );
        assert!(
            events
                .iter()
                .any(|v| v["event"] == "execution" && v["case"] == "reject-orders")
        );
        e.use_case("Supplier", "send-invoice")?;
        let sample = e.sample("Supplier", "Order")?;
        let replies = e.take_deliveries();
        assert_eq!(replies.len(), 2);
        assert_eq!(e.detail(sample)?["findings"].as_array().unwrap().len(), 0);
        let kinds: Vec<_> = replies
            .iter()
            .map(|d| {
                edi::parse(&d.bytes).interchanges[0].messages[0]
                    .kind()
                    .to_owned()
            })
            .collect();
        assert_eq!(kinds, ["CONTRL", "ORDRSP"]);
        e.initiate_case("Supplier", "Invoice")?;
        let invoice = e.take_deliveries().pop().unwrap();
        assert_eq!(
            edi::parse(&invoice.bytes).interchanges[0].messages[0].kind(),
            "INVOIC"
        );
        assert!(e.sample("Supplier", "Missing").is_err());
        assert!(e.use_case("Supplier", "not-installed").is_err());
        drop(e);
        let w = Workspace::load(dir.path())?;
        assert_eq!(w.partners[0].case_id(), Some("send-invoice"));
        let mut e = Engine::open(w, clock)?;
        e.use_case("Supplier", "orders")?;
        input(&mut e, 0, ORDER)?;
        assert_eq!(e.take_deliveries().len(), 2);
        drop(e);
        assert_eq!(
            Workspace::load(dir.path())?.partners[0].case_id(),
            Some("orders")
        );
        Ok(())
    }

    #[test]
    fn invalid_installed_cases_name_the_file() -> Result<()> {
        let (dir, _) = fixture()?;
        install(
            dir.path(),
            include_str!("../../../examples/cases/confirm-orders.json"),
        )?;
        let folder = dir.path().join("cases/confirm-orders");
        fs::write(folder.join("confirm.js"), "function confirm( {")?;
        // An inactive case is checked when it is activated.
        let mut e = Engine::open(Workspace::load(dir.path())?, Arc::new(SystemClock))?;
        let error = format!(
            "{:#}",
            e.use_case("Supplier", "confirm-orders").unwrap_err()
        );
        assert!(error.contains("confirm.js"), "{error}");
        drop(e);
        crate::config::persist_case(dir.path(), "Supplier", "confirm-orders")?;
        let error = format!("{:#}", Workspace::load(dir.path()).unwrap_err());
        assert!(error.contains("confirm.js"), "{error}");
        fs::remove_file(folder.join("confirm.js"))?;
        let error = format!("{:#}", Workspace::load(dir.path()).unwrap_err());
        assert!(
            error.contains("confirm.js")
                && error.contains(
                    &Path::new("cases")
                        .join("confirm-orders")
                        .display()
                        .to_string()
                ),
            "{error}"
        );
        fs::remove_dir_all(&folder)?;
        let error = format!("{:#}", Workspace::load(dir.path()).unwrap_err());
        assert!(
            error.contains("case confirm-orders is not installed"),
            "{error}"
        );
        Ok(())
    }

    const X12_ORDERS: &[u8] = include_bytes!("../../../examples/x12/850-5010.edi");

    fn x12_fixture() -> Result<(tempfile::TempDir, Workspace)> {
        let dir = tempfile::tempdir()?;
        fs::write(
            dir.path().join("vendredi.toml"),
            include_str!("../../../examples/x12/vendredi.toml"),
        )?;
        let case = dir.path().join("cases/orders");
        fs::create_dir_all(&case)?;
        fs::write(
            case.join("case.json"),
            include_bytes!("../../../examples/x12/cases/orders/case.json"),
        )?;
        fs::write(
            case.join("855.edi"),
            include_bytes!("../../../examples/x12/cases/orders/855.edi"),
        )?;
        let w = Workspace::load(dir.path())?;
        Ok((dir, w))
    }

    fn sent(e: &mut Engine) -> Vec<(String, String)> {
        let deliveries = e.take_deliveries();
        for d in &deliveries {
            e.finish_delivery(d.id, Ok(String::new())).unwrap();
        }
        deliveries
            .iter()
            .flat_map(|d| {
                let parsed = interchange::parse(&d.bytes);
                assert!(parsed.findings.is_empty(), "{:?}", parsed.findings);
                parsed
                    .interchanges
                    .iter()
                    .flat_map(|i| i.message_texts().unwrap())
                    .map(|text| {
                        (
                            text[..text.find('*').unwrap_or(text.len())].to_owned(),
                            text,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn x12_partner_acknowledges_groups_and_replies() -> Result<()> {
        let (_dir, w) = x12_fixture()?;
        let clock = Arc::new(Manual(AtomicI64::new(1_767_225_600_000)));
        let mut e = Engine::open(w.clone(), clock.clone())?;
        input(&mut e, 0, X12_ORDERS)?;
        let replies = sent(&mut e);
        let texts: Vec<_> = replies.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            texts,
            [
                "ST*997*0001~AK1*PO*1001*005010~AK2*850*0001~AK5*A~AK2*850*0002~AK5*A~AK9*A*2*2*2~SE*8*0001~",
                "ST*997*0001~AK1*PO*1002*005010~AK2*850*0003~AK5*A~AK9*A*1*1*1~SE*6*0001~",
                "ST*855*0001~BAK*00*AC*PO5001*20261125~SE*3*0001~",
                "ST*855*0001~BAK*00*AC*PO5001*20261125~SE*3*0001~",
                "ST*855*0001~BAK*00*AC*PO5001*20261125~SE*3*0001~",
            ]
        );
        let rows = e.traffic("855")?;
        assert_eq!(rows.len(), 3);
        let inbound_id = e.traffic("in")?[0].id;
        assert_eq!(
            e.detail(inbound_id)?["groups"],
            json!([{"reference":"1001","sets":"850 0001, 850 0002"},{"reference":"1002","sets":"850 0003"}])
        );
        assert_eq!(rows[0].version, "005010");
        let inbound = e.traffic("in")?.pop().unwrap();
        assert_eq!(
            (inbound.kind.as_str(), inbound.reference.as_str()),
            ("850, 850, 850", "000004711")
        );

        // One flawed transaction set is rejected in the 997 and skipped.
        let partial = std::str::from_utf8(X12_ORDERS)?.replacen("SE*9*0002", "SE*8*0002", 1);
        input(&mut e, 0, partial.as_bytes())?;
        let replies = sent(&mut e);
        assert!(
            replies[0].1.contains("AK2*850*0002~AK5*R*4~AK9*P*2*2*1~"),
            "{}",
            replies[0].1
        );
        assert_eq!(replies.len(), 4, "a 997 per group and two 855s");
        assert!(
            e.take_events()
                .iter()
                .any(|v| v["event"] == "rejected_message")
        );

        // An incoming 997 for our 855 marks it acknowledged.
        let ours = e.traffic("855")?[0].reference.clone();
        let control: u64 = ours.parse()?;
        let ack_body = [
            x12::Segment::simple("AK1", &["PR", &control.to_string(), "005010"]),
            x12::Segment::simple("AK9", &["A", "1", "1", "1"]),
        ];
        let ack = x12::envelope(
            &x12::Envelope {
                delimiters: x12::Delimiters::default(),
                sender: x12::Party {
                    qualifier: "ZZ",
                    id: "BUYER",
                    application: "BUYER",
                },
                receiver: x12::Party {
                    qualifier: "ZZ",
                    id: "SUPPLIER",
                    application: "SUPPLIER",
                },
                control: 900,
                group: 900,
                version: "005010",
                usage: 'T',
                ack_requested: false,
                time: "2026-11-25T10:30:00Z".parse()?,
                line_ending: b"\n",
                wrong_count: false,
            },
            &[x12::Outgoing {
                kind: "997",
                reference: "0001",
                implementation: None,
                body: &ack_body,
            }],
        )?;
        input(&mut e, 0, &ack)?;
        assert!(
            e.take_deliveries().is_empty(),
            "acknowledgements are not acknowledged"
        );
        assert!(
            e.take_events()
                .iter()
                .any(|v| v["event"] == "acknowledged" && v["status"] == "accepted")
        );
        assert_eq!(
            e.traffic("855")?
                .iter()
                .find(|r| r.reference == ours)
                .unwrap()
                .status,
            "Acknowledged"
        );

        // EDIFACT sent to an X12 partner is recorded and rejected unanswered.
        let id = input(&mut e, 0, ORDER)?;
        assert!(e.take_deliveries().is_empty());
        assert_eq!(e.detail(id)?["findings"][0]["code"], "standard");
        Ok(())
    }

    #[test]
    fn x12_faults_ta1_999_and_scripts() -> Result<()> {
        let (dir, mut w) = x12_fixture()?;
        let settings = w.partners[0].x12.as_mut().unwrap();
        settings.ta1 = true;
        settings.acknowledgement = GroupAck::ImplementationAck;
        rules(&mut w.partners[0])[0].actions = vec![
            Action::Fault {
                fault: Fault::WrongCount,
            },
            Action::Document {
                document: "cases/orders/855.edi".into(),
            },
        ];
        let mut e = Engine::open(w.clone(), Arc::new(SystemClock))?;
        let one = std::str::from_utf8(X12_ORDERS)?;
        let first = &one[..one
            .find("GS*PO*BUYERAPP*SUPPLIERAPP*20261125*1030*1002")
            .unwrap()];
        let single = format!("{first}IEA*1*000004711~\r\n");
        input(&mut e, 0, single.as_bytes())?;
        let deliveries = e.take_deliveries();
        let kinds: Vec<_> = deliveries
            .iter()
            .map(|d| {
                let p = x12::parse(&d.bytes);
                let i = &p.interchanges[0];
                (
                    i.ta1
                        .first()
                        .map(|s| s.tag.clone())
                        .or_else(|| Some(i.groups[0].transactions[0].kind().to_owned()))
                        .unwrap(),
                    i.findings.iter().map(|f| f.code).collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("TA1".to_owned(), vec![]),
                ("999".to_owned(), vec![]),
                ("855".to_owned(), vec!["ge_count", "iea_count"]),
                ("855".to_owned(), vec!["ge_count", "iea_count"]),
            ]
        );
        assert!(
            x12::parse(&deliveries[1].bytes).interchanges[0].groups[0].transactions[0]
                .segments
                .iter()
                .any(|s| s.tag == "IK5")
        );

        // A rejected interchange gets a rejecting TA1 and 999 and no 855.
        rules(&mut w.partners[0])[0].actions = vec![Action::Fault {
            fault: Fault::Reject,
        }];
        // Close the engine first: Windows keeps open files from being removed.
        drop(e);
        fs::remove_dir_all(dir.path().join(".vendredi"))?;
        let mut e = Engine::open(w.clone(), Arc::new(SystemClock))?;
        input(&mut e, 0, single.as_bytes())?;
        let texts: Vec<_> = sent(&mut e).into_iter().map(|(_, t)| t).collect();
        assert_eq!(texts.len(), 2);
        assert!(
            texts[0].starts_with("TA1*000004711*261125*1030*R*022"),
            "{}",
            texts[0]
        );
        assert!(texts[1].contains("AK9*R*2*2*0"), "{}", texts[1]);

        // Scripts read the X12 view and build structured replies.
        drop(e);
        fs::remove_dir_all(dir.path().join(".vendredi"))?;
        fs::write(
            dir.path().join("reply.js"),
            "function reply(i, api) {
                const g = i.groups[0];
                const po = g.messages[0];
                const beg = po.segments.find(s => s[0] === 'BEG');
                const ref = po.segments.find(s => s[0] === 'REF');
                api.log(g.functional + ' ' + g.reference + ' ' + JSON.stringify(ref[3]));
                api.send(api.build('855', g.version, [['BAK', '00', 'AC', beg[3], beg[5]], ['REF', 'DP', ref[2], ref[3]]]));
            }",
        )?;
        w.partners[0].x12 = Some(Default::default());
        w.partners[0].acknowledge = Policy::Never;
        rules(&mut w.partners[0])[0].actions = vec![Action::Script {
            script: "reply.js".into(),
            function: "reply".into(),
        }];
        let mut e = Engine::open(w, Arc::new(SystemClock))?;
        input(&mut e, 0, single.as_bytes())?;
        let events = e.take_events();
        assert!(events.iter().all(|v| v["error"].is_null()), "{events:#?}");
        assert!(
            events
                .iter()
                .any(|v| v["text"] == "PO 1001 {\"repeat\":[\"Dock\",\"Gate 4\"]}")
        );
        let texts: Vec<_> = sent(&mut e).into_iter().map(|(_, t)| t).collect();
        assert_eq!(
            texts[0],
            "ST*855*0001~BAK*00*AC*PO5001*20261125~REF*DP*038*Dock^Gate 4~SE*4*0001~"
        );
        Ok(())
    }

    #[test]
    fn bad_input_and_failed_script_leave_engine_available() -> Result<()> {
        let (dir, mut w) = fixture()?;
        fs::write(dir.path().join("bad.js"), "function fail(){while(true){}}")?;
        rules(&mut w.partners[0])[0].actions = vec![Action::Script {
            script: "bad.js".into(),
            function: "fail".into(),
        }];
        let mut e = Engine::open(w, Arc::new(SystemClock))?;
        let id = input(&mut e, 0, b"\xffgarbage")?;
        assert_eq!(e.detail(id)?["bytes"][0], 255);
        input(&mut e, 0, ORDER)?;
        assert!(
            e.take_events()
                .iter()
                .any(|v| v["event"] == "action" && v["error"].is_string())
        );
        e.initiate("Supplier", Path::new("confirmation.edi"), None)?;
        assert_eq!(e.take_deliveries().len(), 2);
        Ok(())
    }
}
