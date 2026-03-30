# skill.md — Rust/Tokio Development Skill Guide for Autonomous Agents

## Purpose

Use this skill when building, reviewing, extending, or debugging systems in **Rust** with **Tokio**, especially:
- backend services
- WebSocket / HTTP clients and servers
- event-driven systems
- low-latency services
- trading bots
- CLI tools
- background workers
- replay/simulation engines

This file is written so an autonomous coding agent can use it as an execution guide.

---

## 1. What this skill is for

An agent using this skill should be able to:
- scaffold a Rust project correctly
- choose appropriate crates
- write idiomatic async Rust with Tokio
- separate hot-path logic from slow-path logic
- build reliable services with structured logging and error handling
- add tests, benchmarks, and linting
- reason about ownership/borrowing without fighting the compiler unnecessarily
- produce maintainable, production-friendly code

This skill is optimized for:
- correctness first
- observability second
- performance third
- premature micro-optimization never

---

## 2. Core development principles

1. **Prefer stable Rust**
   - Do not require nightly unless explicitly necessary.
   - Use the edition already configured by the project; otherwise use the latest stable edition available in the local toolchain.

2. **Keep async boundaries clean**
   - Use Tokio only where async I/O or timers are involved.
   - Avoid mixing sync and async carelessly.

3. **Minimize shared mutable state**
   - Prefer ownership, message passing, immutable data, or sharded concurrency.
   - Use locks only when simpler options are worse.

4. **Fail loudly in development, fail safely in production**
   - Propagate rich errors.
   - Avoid silent fallbacks for critical paths.

5. **Keep the hot path small**
   - Parsing, normalization, decision, and execution should be lean.
   - Heavy analytics, formatting, and file/network writes belong off the hot path.

6. **Reproducibility matters**
   - Make behavior replayable via logs or captured event streams.
   - Keep config versioned and explicit.

---

## 3. Rust mental model for the agent

### Ownership
- Every value has one owner by default.
- Move values unless borrowing is enough.
- Prefer borrowing (`&T`) for read-only access.
- Use mutable borrows (`&mut T`) sparingly and locally.

### Borrowing
- Use short borrow scopes.
- When borrow checker friction appears:
  - reduce variable lifetimes
  - split functions
  - clone small cheap values if it simplifies correctness
  - move logic into smaller blocks

### Error handling
- Use `Result<T, E>` for fallible operations.
- Use `?` for propagation.
- Prefer concrete, structured error enums in library code.
- Prefer `anyhow` in top-level application code if needed.
- Never `unwrap()` in production paths unless logically impossible and documented.

### Pattern matching
- Use `match` for protocol/state handling.
- Prefer enums for finite state machines and message types.

### Types
- Model domain concepts with strong types.
- Avoid using raw `String`, `u64`, or `f64` everywhere if domain-specific wrappers improve safety.

---

## 4. Tokio mental model

Tokio is the async runtime.

Use Tokio for:
- WebSockets
- HTTP clients/servers
- timers
- task scheduling
- channels
- cancellation
- graceful shutdown

### Prefer
- `tokio::spawn` for concurrent tasks
- `tokio::select!` for racing events
- `tokio::time::timeout` for bounded waits
- `tokio::sync::{mpsc, broadcast, watch}` for task communication
- `CancellationToken` if available via `tokio-util` for coordinated shutdown

### Avoid
- blocking CPU-heavy work inside async tasks
- calling blocking filesystem/network code from async tasks
- large critical sections under async mutexes
- uncontrolled task spawning

### Rule of thumb
- **I/O-bound work** -> Tokio async task
- **CPU-heavy work** -> dedicated thread pool / `spawn_blocking` / separate worker

---

## 5. Preferred project structure

For an application:

```text
project/
  Cargo.toml
  Cargo.lock
  README.md
  .env.example
  rust-toolchain.toml            # optional
  src/
    main.rs
    lib.rs
    config.rs
    error.rs
    types.rs
    metrics.rs
    shutdown.rs
    app.rs
    domain/
      mod.rs
      ...
    infra/
      mod.rs
      http.rs
      ws.rs
      storage.rs
    services/
      mod.rs
      ...
    bin/
      ...
  tests/
    integration_*.rs
  benches/
    ...
  examples/
    ...
  scripts/
    ...
```

For a larger system, prefer a workspace:

```text
workspace/
  Cargo.toml
  crates/
    core/
    api/
    ws/
    engine/
    cli/
```

### Guidance
- Put reusable domain logic in `lib.rs`.
- Keep `main.rs` thin: config, wiring, startup, shutdown.
- Split domain logic from infrastructure logic.
- Never bury all logic in one huge `main.rs`.

---

## 6. Recommended crate choices

Use only what is necessary.

### Common baseline crates
- `tokio` — async runtime
- `serde`, `serde_json` — serialization
- `thiserror` — typed error enums
- `tracing`, `tracing-subscriber` — structured logs
- `reqwest` — HTTP client
- `tokio-tungstenite` — WebSocket client
- `futures` / `futures-util` — stream/sink helpers
- `clap` — CLI
- `dotenvy` — local env loading
- `toml` / `config` — config files if needed
- `uuid` — IDs if needed
- `chrono` or `time` — timestamps
- `rust_decimal` — money/price precision when floats are unsafe
- `dashmap` — concurrent map when truly needed
- `parking_lot` — fast sync locks for non-async contexts
- `anyhow` — app-level error aggregation
- `tokio-util` — codec/cancellation helpers
- `bytes` — network buffers
- `simd-json` — only when parsing speed is a real bottleneck and compatibility is acceptable

### Testing and quality
- `tokio-test`
- `proptest` — property-based tests
- `wiremock` or mock server libraries if HTTP mocking is needed
- `tempfile`
- `criterion` — benchmarks

### Avoid adding crates when
- stdlib is enough
- the dependency is unmaintained
- the dependency solves only a tiny convenience issue

---

## 7. Configuration rules

### Preferred config order
1. defaults in code
2. config file if used
3. environment variables
4. CLI overrides

### Requirements
- validate config at startup
- fail fast on missing critical config
- provide `.env.example`
- separate development and production config clearly
- never hardcode secrets

### Example config domains
- network endpoints
- credentials
- feature flags
- timeouts
- retry policy
- log level
- rate limits
- risk limits
- storage paths

---

## 8. Logging and observability

Use `tracing`, not ad-hoc `println!`, except for quick experiments.

### Must-have logging practices
- include request/task IDs where possible
- log key state transitions
- log retries with reason and attempt count
- log shutdown/startup clearly
- avoid spamming logs inside tight loops

### Use structured fields
Prefer:
```rust
tracing::info!(market = %market_id, side = ?side, "placing order");
```

Instead of:
```rust
println!("placing order on {}", market_id);
```

### Metrics
If metrics are needed, expose:
- latency histograms
- error counts
- reconnect counts
- queue depths
- event throughput
- success/failure counters

---

## 9. Error-handling policy

### Application code
Use:
- `thiserror` for internal typed errors
- `anyhow` at app boundaries if it simplifies orchestration

### Library code
- prefer typed errors
- avoid `anyhow` in public library APIs

### Requirements
- preserve source errors
- annotate context on network/storage failures
- distinguish retryable vs non-retryable errors
- never swallow errors silently

### Good pattern
- parse -> validate -> map domain error -> propagate with context

---

## 10. Async architecture patterns

### Pattern A: Pipeline
Use when flow is linear.

Example:
- ingest
- parse
- normalize
- compute
- emit

### Pattern B: Actor/task model
Use when components are independent and communicate through channels.

Example:
- websocket task
- signal engine task
- execution task
- telemetry task

### Pattern C: Shared state + subscriptions
Use sparingly for dashboards, status feeds, and config hot reload.

### Agent guidance
Prefer:
- channels for ownership clarity
- explicit state machines for lifecycle-heavy components
- `select!` loops for orchestrators

Avoid:
- one giant mutable global state object
- nested async locks
- hidden side effects across modules

---

## 11. Concurrency and state management

### Use channels first
Choose:
- `mpsc` for work queues
- `broadcast` for fan-out notifications
- `watch` for latest-state snapshots

### Use locks only when needed
If using `Mutex`:
- keep lock duration tiny
- do not hold lock across `.await`

If using `RwLock`:
- only if reads heavily outnumber writes and complexity is justified

### State design preference
1. immutable messages
2. single owner task
3. concurrent map / lock
4. global mutable singleton only as last resort

---

## 12. Networking guidelines

### HTTP clients
- reuse client instances
- set connect/read/request timeouts
- implement retry with backoff and jitter only for safe operations
- distinguish idempotent from non-idempotent requests
- bound concurrency

### WebSocket clients
- reconnect with backoff
- detect stale connections
- support ping/pong or heartbeat
- separate read loop from message processing if needed
- preserve ordering requirements explicitly

### Parsing
- parse into typed structs where reasonable
- validate protocol invariants
- reject malformed payloads loudly in development
- do not trust external input

---

## 13. Precision and numeric rules

For:
- money
- prices
- fees
- quantities
- probabilities where precision matters

Prefer:
- integers in smallest unit, or
- `rust_decimal`

Avoid:
- `f32`
- casual `f64` use for financial logic

When `f64` is used:
- document why
- bound numerical assumptions
- avoid equality comparisons

---

## 14. Performance guidelines

### First rule
Measure before optimizing.

### Safe optimization priorities
1. remove unnecessary allocations
2. reduce copying
3. reduce logging in tight loops
4. reuse buffers/clients
5. reduce lock contention
6. split hot path from cold path

### Do not
- optimize readability away prematurely
- introduce unsafe code without strong reason
- use lock-free structures unless measured need exists

### Hot-path checklist
- no blocking I/O
- no expensive string formatting
- no per-event client construction
- no unnecessary heap churn
- bounded memory growth

---

## 15. Testing strategy

Every serious Rust/Tokio project should include:

### Unit tests
Test:
- pure domain logic
- parsing/validation
- state transitions
- edge cases

### Integration tests
Test:
- config loading
- network adapters
- storage integration
- end-to-end component behavior

### Async tests
Use `#[tokio::test]` where necessary.

### Property tests
Use for:
- parsers
- invariants
- calculations
- state machine transitions

### Replay tests
For event-driven systems, store sample inputs and replay them deterministically.

### Failure tests
Test:
- disconnects
- timeouts
- retries
- partial failures
- cancellation
- restart behavior

---

## 16. Benchmarking rules

Use benchmarks only where value exists.

### Benchmark these first
- parsing throughput
- event normalization
- decision logic
- serialization/deserialization
- queue/channel pressure points

### Use
- `criterion` for microbenchmarks
- application-level timing for end-to-end stages

### Report
- baseline
- change
- workload
- hardware context

---

## 17. Code quality gates

Before calling work done, run:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

If relevant, also run:
```bash
cargo check
cargo bench
cargo doc --no-deps
```

### Quality bar
- formatting clean
- lint warnings resolved or justified
- tests pass
- no debug leftovers
- no dead code unless intentional and documented

---

## 18. Documentation rules

The agent should produce:
- `README.md`
- `.env.example`
- inline docs for non-obvious modules
- comments for tricky ownership/lifetime logic
- operational instructions for running locally

### README should include
- what it does
- how to run
- config required
- project structure
- test commands
- known limitations

### Comment only when needed
Comment:
- why
- invariants
- protocol quirks
- safety assumptions

Do not comment obvious syntax.

---

## 19. Security rules

- never commit secrets
- use environment variables or secret managers
- validate all external input
- sanitize logs if secrets or PII may appear
- separate dev/test/prod credentials
- use TLS endpoints unless explicitly impossible
- avoid shelling out unless necessary
- audit dependencies if project is high-stakes

For signing/keys:
- keep secret material out of logs
- minimize lifetime of sensitive values in memory when practical
- isolate signing logic into a dedicated module

---

## 20. Agent workflow

When asked to build in Rust/Tokio, follow this order:

### Step 1: clarify the target
Identify:
- CLI, server, bot, or library
- sync vs async
- latency sensitivity
- persistence needs
- external dependencies

### Step 2: scaffold correctly
Create:
- Cargo project/workspace
- module layout
- config
- logging
- error types

### Step 3: build the happy path
Implement:
- typed models
- protocol adapters
- core logic
- basic tests

### Step 4: add resilience
Implement:
- retries
- timeouts
- reconnection
- shutdown
- bounded queues
- validation

### Step 5: add observability
Implement:
- tracing
- key metrics
- clear startup/shutdown logs

### Step 6: harden
Add:
- integration tests
- replay tests
- performance checks
- docs
- clippy cleanup

---

## 21. Recommended delivery format for generated code

When generating code, the agent should prefer:
- complete files over fragments when possible
- small modules instead of one giant file
- compile-ready code
- reasonable comments
- explicit error handling
- testable function boundaries

If a system is large, generate in this order:
1. `Cargo.toml`
2. `main.rs` / `lib.rs`
3. config and types
4. core modules
5. adapters
6. tests
7. README

---

## 22. Common anti-patterns to avoid

- giant `main.rs`
- business logic inside WebSocket read loop
- holding mutex across `.await`
- `unwrap()` in networked production paths
- ad-hoc stringly typed protocols
- using floats for financial logic without justification
- unbounded channels in high-throughput pipelines
- reconnect loops without backoff
- one task doing ingest + parse + compute + I/O + logging + persistence
- mixing domain models with raw transport payloads everywhere

---

## 23. Suggested defaults for event-driven services

These are sane defaults, not strict rules.

- async runtime: `tokio`
- logging: `tracing`
- config: env + `.env.example`
- serialization: `serde`
- HTTP: `reqwest` with reused client
- WS: `tokio-tungstenite`
- errors: `thiserror` + `anyhow`
- precision: `rust_decimal` or integer fixed units
- tests: unit + integration + replay
- shutdown: cancellation token + signal handling
- formatting/linting: `fmt` + `clippy`

---

## 24. Commands cheat sheet

### New project
```bash
cargo new my_app
cd my_app
```

### Check and run
```bash
cargo check
cargo run
```

### Release build
```bash
cargo build --release
```

### Tests
```bash
cargo test
```

### Lint and format
```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
```

### Docs
```bash
cargo doc --no-deps --open
```

### Benchmarks
```bash
cargo bench
```

---

## 25. Definition of done

A Rust/Tokio task is not done until:
- code compiles
- formatting passes
- lints pass or are explicitly justified
- tests pass
- config is documented
- failure modes are considered
- logs are useful
- no unsafe assumptions are hidden
- README explains how to run and verify the result

---

## 26. Special guidance for low-latency / trading-style systems

If the project is a low-latency or event-driven trading system:

### Must do
- separate hot path from cold path
- timestamp on receipt
- use precise numeric types
- keep order/state lifecycle explicit
- log all decisions with enough context for replay
- make stale-data checks first-class
- keep REST out of hot path when WebSocket is available
- keep order submission idempotent where possible
- reconcile state after restart

### Should do
- benchmark p50/p95/p99 internal stages
- reuse buffers and client instances
- use append-only local logs for replay
- keep risk guards outside strategy logic

### Must not do
- assume paper trading equals live execution
- infer financial truth from approximate floats
- submit duplicate orders without explicit policy
- silently fall back to a fake/default market in production

---

## 27. Output contract for autonomous agents

When using this skill, the agent should output:
1. a concise plan
2. compile-ready Rust/Tokio code
3. config/setup instructions
4. tests
5. explanation of important tradeoffs
6. a list of follow-up hardening tasks if the build is only v1

If something cannot be completed, the agent should state exactly:
- what is missing
- what is assumed
- what remains risky

---

## 28. Final guidance

Rust/Tokio development should feel:
- explicit
- testable
- observable
- safe
- boring in the best way

When in doubt:
- simplify ownership
- reduce shared state
- make messages explicit
- add tests
- measure before optimizing
