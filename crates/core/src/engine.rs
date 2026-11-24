use crate::{
    config::*,
    edi::{self, Finding, Interchange, Message},
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
        self.store.transaction(|s|{
            let roots="SELECT root FROM transfers GROUP BY root HAVING MAX(time) < ?";
            let transfers=format!("SELECT id FROM transfers WHERE root IN ({roots})");
            for sql in [
                format!("DELETE FROM events WHERE json_extract(body,'$.root') IN ({roots})"),
                format!("DELETE FROM events WHERE json_extract(body,'$.root') IS NULL AND json_extract(body,'$.execution') IN (SELECT id FROM executions WHERE root IN ({roots}))"),
                format!("DELETE FROM findings WHERE transfer IN ({transfers})"),
                format!("DELETE FROM messages WHERE interchange IN (SELECT id FROM interchanges WHERE transfer IN ({transfers}))"),
                format!("DELETE FROM interchanges WHERE transfer IN ({transfers})"),
                format!("DELETE FROM executions WHERE root IN ({roots})"),
                format!("DELETE FROM transfers WHERE root IN ({roots})"),
            ] {s.exec(&sql,[cutoff])?;}
            Ok(())
        })
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
        self.store
            .query(include_str!("traffic.sql"), [search])?
            .iter()
            .map(|r| {
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
                Ok(TrafficRow {
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
                })
            })
            .collect()
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
        self.process(p, replay, edi::parse(&bytes))
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
        let findings=self.store.query("SELECT severity,code,offset,text FROM findings WHERE transfer=? ORDER BY offset,id",[id])?
            .iter().map(|r|Ok(json!({"severity":r.get::<String>(0)?,"code":r.get::<String>(1)?,"offset":r.get::<i64>(2)?,"text":r.get::<String>(3)?}))).collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"transfer":id,"segments":edi::lines(&bytes),"findings":findings,"interchanges":summaries,"status":r.get::<String>(2)?,"metadata":r.get::<String>(1)?,"bytes":bytes,"raw":String::from_utf8_lossy(&bytes),"parsed":edi::parse(&bytes),"events":events}),
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
    pub fn receipt(&self, p: usize, parsed: &edi::Parsed) -> Receipt {
        for m in parsed.interchanges.iter().flat_map(|i| &i.messages) {
            if let Some(rule) = self.workspace.partners[p]
                .rules
                .iter()
                .find(|r| r.matches(m))
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
        let h = &i.header;
        let id=s.exec("INSERT INTO interchanges(transfer,sender,recipient,reference,test_flag,ack_requested,ack_status) VALUES (?,?,?,?,?,?,'unknown')",
        (transfer,h.at(1,0),h.at(2,0),h.at(4,0),h.at(10,0),h.at(8,0)))?;
        let mut messages = Vec::new();
        for m in &i.messages {
            let h = m.header();
            messages.push(s.exec("INSERT INTO messages(interchange,type,version,release,reference,segment_count) VALUES (?,?,?,?,?,?)",
            (id,m.kind(),h.at(1,1),h.at(1,2),h.at(0,0),m.segments.len() as i64))?);
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
        self.event(
            "execution",
            root,
            json!({"execution":execution,"name":name,"interchange":trigger.0,"message":trigger.1}),
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
    pub fn process(&mut self, p: usize, id: i64, parsed: edi::Parsed) -> Result<()> {
        self.findings(id, None, &parsed.findings)?;
        for mut i in parsed.interchanges {
            let partner = &self.workspace.partners[p];
            if i.header.elements.get(1) != Some(&self.workspace.identity)
                || i.header.elements.get(2) != Some(&partner.identity)
            {
                i.findings.push(Finding::new(
                    "identity",
                    i.header.offset,
                    "UNB identities do not match receive channel",
                ));
            }
            if i.header.at(0, 0) != partner.charset {
                i.findings.push(Finding::new(
                    "charset",
                    i.header.offset,
                    "unexpected character set",
                ));
            }
            let (iid, mids) = self.store.transaction(|s| Self::summary(s, id, &i))?;
            let rules: Vec<_> = i
                .messages
                .iter()
                .map(|m| partner.rules.iter().find(|r| r.matches(m)).cloned())
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
            let ack = partner.acknowledge == Policy::Always
                || (partner.acknowledge == Policy::Requested && i.header.at(8, 0) == "1");
            let accepted = i.findings.is_empty() && !faults.contains(&Fault::Reject);
            self.findings(id, Some(iid), &i.findings)?;
            self.event("validated",Some(id),json!({"interchange":iid,"accepted":accepted,"messages":mids.len(),"findings":i.findings.len()}))?;
            let i = Arc::new(i);
            if ack
                && !faults.contains(&Fault::SuppressAck)
                && !i.messages.iter().any(|m| m.kind() == "CONTRL")
                && !i.header.at(4, 0).is_empty()
            {
                let mut t = self.task(
                    p,
                    Some(id),
                    (Some(iid), None),
                    "acknowledge",
                    vec![],
                    Some(i.clone()),
                )?;
                self.send(&mut t, &edi::contrl(&i, accepted))?;
            }
            if !accepted {
                continue;
            }
            for ((m, mid), rule) in i.messages.iter().zip(mids).zip(rules) {
                if m.kind() == "CONTRL" {
                    self.correlate(p, id, m)?;
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
                    self.event("no_match", Some(id), json!({"message":mid,"type":m.kind()}))?;
                }
            }
        }
        Ok(())
    }
    fn correlate(&mut self, p: usize, root: i64, m: &Message) -> Result<()> {
        let Some(uci) = m.segments.iter().find(|s| s.tag == "UCI") else {
            return Ok(());
        };
        let partner = &self.workspace.partners[p];
        let rows=self.store.query("SELECT i.id FROM interchanges i JOIN transfers t ON t.id=i.transfer WHERE t.direction='out' AND t.partner=? AND i.reference=?",
            (partner.name.as_str(),uci.at(0,0)))?;
        if rows.len() == 1
            && uci.elements.get(1) == Some(&partner.identity)
            && uci.elements.get(2) == Some(&self.workspace.identity)
        {
            let iid = rows[0].get::<i64>(0)?;
            let status = match uci.at(3, 0) {
                "7" | "8" => "accepted",
                "4" => "rejected",
                _ => "unknown",
            };
            self.store.exec(
                "UPDATE interchanges SET ack_status=? WHERE id=?",
                (status, iid),
            )?;
            self.event(
                "acknowledged",
                Some(root),
                json!({"interchange":iid,"status":status}),
            )?;
        } else {
            self.event(
                "unresolved_ack",
                Some(root),
                json!({"reference":uci.at(0,0),"candidates":rows.len()}),
            )?;
        }
        Ok(())
    }
    fn send(&mut self, t: &mut Task, m: &Message) -> Result<()> {
        let partner = &self.workspace.partners[t.partner];
        let name = partner.name.as_str();
        let now = self.clock.millis();
        let (id,bytes,root)=self.store.transaction(|s|{
            let last=s.query("SELECT reference FROM partner_state WHERE partner=?",[name])?[0].get::<i64>(0)?;
            let next=if t.faults.contains(&Fault::DuplicateReference) && last>0 {last} else {last.checked_add(1).context("reference counter exhausted")?};
            let mut bytes=edi::envelope(&edi::Envelope {syntax:partner.syntax()?,charset:&partner.charset,sender:&partner.identity,recipient:&self.workspace.identity,reference:&next.to_string(),
                time:DateTime::from_timestamp_millis(now).context("invalid clock")?,wrong_count:t.faults.contains(&Fault::WrongCount)},m)?;
            if t.faults.contains(&Fault::Malformed){bytes.pop();}
            s.exec("UPDATE partner_state SET reference=? WHERE partner=?",(next,name))?;
            let metadata = partner.send.metadata();
            let id=s.exec("INSERT INTO transfers(id,root,time,direction,partner,metadata,bytes,status,execution) VALUES (?,?,?,'out',?,?,?,'pending',?)",
                (Self::allocate_transfer(s)?,t.root,now,name,metadata.to_string(),bytes.clone(),t.execution))?;
            let root=t.root.unwrap_or(id);
            if t.root.is_none(){s.exec("UPDATE transfers SET root=? WHERE id=?",(root,id))?;s.exec("UPDATE executions SET root=? WHERE id=?",(root,t.execution))?;}
            if let Some(i)=edi::parse(&bytes).interchanges.first(){Self::summary(s,id,i)?;}
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
                let m = edi::document(&fs::read(self.workspace.root.join(document))?)?;
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
                let inbound = serde_json::to_value(t.inbound.as_deref())?;
                let effects = script::run(
                    &fs::read_to_string(self.workspace.root.join(path))?,
                    function,
                    &script::Input {
                        interchange: &inbound,
                        state: &state,
                        reference,
                        now: self.clock.millis(),
                    },
                )?;
                let messages = effects
                    .sends
                    .iter()
                    .map(|raw| edi::script_document(raw))
                    .collect::<Result<Vec<_>>>()?;
                ensure!(
                    !effects.acknowledge || t.inbound.is_some(),
                    "CONTRL needs an inbound interchange"
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
                if effects.acknowledge {
                    let m = edi::contrl(t.inbound.as_ref().unwrap(), true);
                    self.send(t, &m)?;
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
        fs::write(
            dir.path().join("vendredi.toml"),
            include_str!("../../../examples/orders/vendredi.toml"),
        )?;
        fs::write(
            dir.path().join("confirmation.edi"),
            include_bytes!("../../../examples/orders/confirmation.edi"),
        )?;
        let w = Workspace::load(dir.path())?;
        Ok((dir, w))
    }
    fn input(e: &mut Engine, p: usize, bytes: &[u8]) -> Result<i64> {
        let id = e.receive(p, bytes, json!({}), None)?.unwrap();
        e.process(p, id, edi::parse(bytes))?;
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
            engine.process(0, id, edi::parse(ORDER))?;
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
    fn partner_delay_script_state_and_faults_are_isolated() -> Result<()> {
        let (dir, mut w) = fixture()?;
        fs::write(
            dir.path().join("handle.js"),
            "function run(i,a){a.set('calls',(a.get('calls')||0)+1);a.log(a.now());a.send(a.build('ORDRSP:D:96A:UN', \"BGM+231'\"))}",
        )?;
        let rule = &mut w.partners[0].rules[0];
        rule.actions = vec![
            Action::Delay { delay_ms: 20 },
            Action::Script {
                script: "handle.js".into(),
                function: "run".into(),
            },
        ];
        let mut other = w.partners[0].clone();
        other.name = "Other".into();
        other.rules[0].actions = vec![
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
    #[test]
    fn bad_input_and_failed_script_leave_engine_available() -> Result<()> {
        let (dir, mut w) = fixture()?;
        fs::write(dir.path().join("bad.js"), "function fail(){while(true){}}")?;
        w.partners[0].rules[0].actions = vec![Action::Script {
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
