use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    thread,
};
use vendredi_core::{
    config::Workspace,
    edi,
    engine::{Delivery, Engine, HistoryPeriod, Receipt, SystemClock},
};
use vendredi_transports::{self as transport, Inbound};

pub enum Command {
    Receive(Inbound),
    Delivered(i64, Result<String, String>),
    Send(String, PathBuf, Option<String>),
    Resend(i64),
    Inspect(i64),
    Traffic(String),
    ClearHistory(HistoryPeriod, Option<String>),
    Listen(bool),
    Replay(i64),
    Stop,
}
pub type Output = Arc<dyn Fn(Value) + Send + Sync>;
pub struct Running {
    pub tx: Sender<Command>,
    thread: Option<thread::JoinHandle<Result<()>>>,
}
impl Running {
    pub fn stop(&mut self) -> Result<()> {
        let _ = self.tx.send(Command::Stop);
        self.wait()
    }
    pub fn wait(&mut self) -> Result<()> {
        if let Some(t) = self.thread.take() {
            t.join()
                .map_err(|_| anyhow!("engine thread stopped unexpectedly"))??;
        }
        Ok(())
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub fn start(w: Workspace, out: Output, with_history: bool, listen: bool) -> Result<Running> {
    let (tx, rx) = mpsc::channel();
    let (ready, started) = mpsc::sync_channel(1);
    let address = w.listen;
    let input = tx.clone();
    let delivered = tx.clone();
    let root = w.root.clone();
    let handle = thread::spawn(move || {
        let ingest: transport::Ingest = Arc::new(move |i| {
            input
                .send(Command::Receive(i))
                .map_err(|_| anyhow!("engine stopped"))
        });
        let setup = (|| -> Result<_> {
            let e = Engine::open(w, Arc::new(SystemClock))?;
            let listeners = if listen {
                Some(transport::listen(&e.workspace, ingest.clone())?)
            } else {
                None
            };
            Ok((e, listeners))
        })();
        let (mut e, mut listeners) = match setup {
            Ok(v) => v,
            Err(err) => {
                let _ = ready.send(Err(format!("{err:#}")));
                return Err(err);
            }
        };
        let (transport_tx, transport_rx) = mpsc::channel::<Delivery>();
        thread::spawn(move || {
            for d in transport_rx {
                let result = transport::deliver(&root, &d).map_err(|err| format!("{err:#}"));
                if delivered.send(Command::Delivered(d.id, result)).is_err() {
                    break;
                }
            }
        });
        if with_history {
            let rows = e.traffic("")?;
            let focus = rows
                .iter()
                .find(|r| !r.outgoing)
                .or_else(|| rows.first())
                .map(|r| r.id);
            out(json!({"event":"traffic","search":"","rows":rows}));
            if let Some(id) = focus {
                out(json!({"event":"detail","value":e.detail(id)?}));
            }
            for v in e.history(500)? {
                out(v);
            }
        }
        out(json!({"event":"ready","listen":address.to_string()}));
        ready
            .send(Ok(()))
            .map_err(|_| anyhow!("launcher disconnected"))?;
        let mut in_flight = 0usize;
        let mut stopping = false;
        loop {
            let cmd = match e.next_wait() {
                Some(wait) => match rx.recv_timeout(wait) {
                    Ok(c) => Some(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(_) => break,
                },
                None => rx.recv().ok(),
            };
            let result = match cmd {
                Some(Command::Receive(i)) => receive(&mut e, i),
                Some(Command::Delivered(id, result)) => {
                    in_flight -= 1;
                    e.finish_delivery(id, result)
                }
                Some(Command::Send(p, path, f)) => e.initiate(&p, &path, f.as_deref()),
                Some(Command::Resend(id)) => e.resend(id),
                Some(Command::Replay(id)) => e.replay(id),
                Some(Command::Traffic(search)) => e
                    .traffic(&search)
                    .map(|rows| out(json!({"event":"traffic","search":search,"rows":rows}))),
                Some(Command::ClearHistory(period, partner)) => e
                    .clear_history(period, partner.as_deref())
                    .map(|count| out(json!({"event":"history_cleared", "count":count}))),
                Some(Command::Listen(enable)) => {
                    let result = if enable && listeners.is_none() {
                        transport::listen(&e.workspace, ingest.clone()).map(|l| listeners = Some(l))
                    } else {
                        if !enable {
                            listeners.take();
                        }
                        Ok(())
                    };
                    out(json!({"event":"listening","active":listeners.is_some()}));
                    result
                }
                Some(Command::Inspect(id)) => e
                    .detail(id)
                    .map(|v| out(json!({"event":"detail","value":v}))),
                Some(Command::Stop) => {
                    stopping = true;
                    Ok(())
                }
                None => Ok(()),
            };
            if let Err(err) = result {
                out(json!({"event":"error","error":format!("{err:#}")}));
            }
            e.tick()?;
            for event in e.take_events() {
                out(event);
            }
            for d in e.take_deliveries() {
                transport_tx.send(d)?;
                in_flight += 1;
            }
            if stopping && in_flight == 0 {
                break;
            }
        }
        Ok(())
    });
    started.recv()?.map_err(|e| anyhow!("{e}"))?;
    Ok(Running {
        tx,
        thread: Some(handle),
    })
}

fn receive(e: &mut Engine, i: Inbound) -> Result<()> {
    let id = match e.receive(i.partner, &i.bytes, i.metadata, i.claim.as_deref()) {
        Ok(Some(id)) => id,
        Ok(None) => {
            let _ = i.recorded.send(Ok(Receipt::Accepted));
            return Ok(());
        }
        Err(err) => {
            let _ = i.recorded.send(Err(format!("{err:#}")));
            return Err(err);
        }
    };
    let parsed = edi::parse(&i.bytes);
    let _ = i.recorded.send(Ok(e.receipt(i.partner, &parsed)));
    e.process(i.partner, id, parsed)
}
