# Metal timestamp feasibility probe

This headless tool tests timestamp support and readback ordering on the same
wgpu version as Ushas. It does **not** render a Bevy frame, invoke MetalFX, or
validate a governor input. See [the feasibility verdict](../../docs/research/timing-feasibility.md).

```sh
cargo test --locked --manifest-path tools/timing-probe/Cargo.toml
cargo run --locked --manifest-path tools/timing-probe/Cargo.toml -- --capabilities-only
cargo run --locked --manifest-path tools/timing-probe/Cargo.toml -- --deferred-resolve --pass-descriptors-only
```

macOS GPU access may require execution outside a restricted sandbox. A failure
to discover any Metal adapter inside a sandbox does not establish lack of
hardware support.

The default run executes 16 synthetic samples: pass-descriptor and encoder
timestamp modes, light/heavy dependent compute chains, and zero/20 ms
pre-submit CPU-delay controls. Each sample contains three command buffers that
read/write the same storage buffer, followed by query resolution and readback.
The final storage value is checked against an integer CPU reference to reject
an empty or incorrect workload. Each readback has a unique encoded sequence ID.

`--deferred-resolve` waits for the workload submission to complete before
submitting query resolution. This is a bounded diagnostic control with a
10-second completion deadline. An eventual runtime integration must pipeline
completion and resolution across frames without waiting in the render loop.

The default encoder timestamp case reproduced invalid queries on M5 Max /
macOS 26.5.2. `--pass-descriptors-only` omits that case. The program emits JSONL
including raw queries and exits nonzero if any query or workload result is
invalid. Exit zero establishes only that this synthetic probe passed.

The committed evidence files retain the initial failures and the successful
deferred pass-descriptor control. [evidence.json](evidence.json) records exact
commands, exit outcomes, and the earlier mixed run's incomplete output.

