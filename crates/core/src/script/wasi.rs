//! WASI Preview 1 calls used by the bundled Rust standard library and Boa.
//! The linker rejects any additional imports at module initialization.
use super::Limits;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use wasmtime::{Caller, Linker, Memory};
use wasmtime::{Error, Result};

type Guest<'a> = Caller<'a, Limits>;

fn memory(caller: &mut Guest<'_>) -> Result<Memory> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| Error::msg("missing guest memory"))
}

pub fn link(linker: &mut Linker<Limits>) -> Result<()> {
    const WASI: &str = "wasi_snapshot_preview1";
    const BADF: i32 = 8;
    const INVAL: i32 = 28;
    const NOTSUP: i32 = 58;
    linker.func_wrap(WASI, "environ_get", |_: u32, _: u32| 0i32)?;
    linker.func_wrap(
        WASI,
        "environ_sizes_get",
        |mut c: Guest, count: u32, size: u32| -> Result<i32> {
            let mem = memory(&mut c)?;
            mem.write(&mut c, count as usize, &[0; 4])?;
            mem.write(&mut c, size as usize, &[0; 4])?;
            Ok(0)
        },
    )?;
    linker.func_wrap(
        WASI,
        "random_get",
        |mut c: Guest, ptr: u32, len: u32| -> Result<i32> {
            // Hash seeding and Math.random need a small seed.
            if len > 256 {
                return Ok(INVAL);
            }
            let mut seed = [0; 256];
            let bytes = &mut seed[..len as usize];
            getrandom::fill(bytes).map_err(|e| Error::msg(format!("random seed: {e}")))?;
            memory(&mut c)?.write(&mut c, ptr as usize, bytes)?;
            Ok(0)
        },
    )?;
    let start = Instant::now();
    linker.func_wrap(
        WASI,
        "clock_time_get",
        move |mut c: Guest, id: u32, _: u64, ptr: u32| -> Result<i32> {
            let elapsed = match id {
                0 => SystemTime::now().duration_since(UNIX_EPOCH)?,
                1 => start.elapsed(),
                _ => return Ok(INVAL),
            };
            memory(&mut c)?.write(
                &mut c,
                ptr as usize,
                &(elapsed.as_nanos() as u64).to_le_bytes(),
            )?;
            Ok(0)
        },
    )?;
    linker.func_wrap(WASI, "fd_write", |_: i32, _: u32, _: u32, _: u32| BADF)?;
    linker.func_wrap(WASI, "poll_oneoff", |_: u32, _: u32, _: u32, _: u32| NOTSUP)?;
    linker.func_wrap(WASI, "sched_yield", || 0i32)?;
    linker.func_wrap(WASI, "proc_exit", |code: i32| -> Result<()> {
        Err(Error::msg(format!("script guest exited ({code})")))
    })?;
    Ok(())
}
