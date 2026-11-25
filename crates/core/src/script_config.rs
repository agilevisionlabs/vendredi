// Shared by the build-time compiler and the runtime that loads its artifact.
pub const MEMORY_LIMIT: usize = 32 * 1024 * 1024;

pub fn engine() -> wasmtime::Result<wasmtime::Engine> {
    let mut config = wasmtime::Config::new();
    // Vendredi ships on little-endian 64-bit platforms. Pulley bytecode runs
    // on all of them without executable pages or a compiler in the binary.
    config
        .target("pulley64")?
        .consume_fuel(true)
        .signals_based_traps(false)
        .memory_guard_size(0)
        .memory_reservation(MEMORY_LIMIT as u64)
        .memory_init_cow(false)
        .generate_address_map(false);
    wasmtime::Engine::new(&config)
}
