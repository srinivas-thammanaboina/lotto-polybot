# CLAUDE.md

## Quality commands
```bash
cargo fmt                                                    # format
cargo clippy --all-targets --all-features -- -D warnings     # lint
cargo test                                                   # test
cargo bench                                                  # benchmark
bash scripts/check.sh                                        # all checks
```

## Rules
- Default mode is `simulation`, never `live`
- Use `rust_decimal` for money/price/fee values, not f64
- Use `thiserror` for typed errors, `anyhow` at app boundaries only
- Use `tracing` for logs, never `println!` in production paths
- No blocking I/O in the hot path
- No `unwrap()` in production paths
- Keep `main.rs` thin — wiring only
