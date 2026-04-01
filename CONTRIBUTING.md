# Contributing to quant-cache

Thank you for your interest in contributing to quant-cache!

## Getting Started

```bash
git clone https://github.com/albert-einshutoin/quant-cache.git
cd quant-cache
cargo build --workspace
cargo test --workspace
```

## Development Workflow

1. Fork the repository
2. Create a feature branch (`git checkout -b feat/my-feature`)
3. Make your changes
4. Run checks:
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test --workspace
   ```
5. Commit with a descriptive message (`feat: add X`, `fix: resolve Y`)
6. Open a Pull Request

## Code Style

- Follow `rustfmt` defaults (configured in `rustfmt.toml`)
- All clippy warnings must be resolved
- Add tests for new functionality
- Keep functions focused (< 50 lines)

## Architecture

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for crate responsibilities and data flow.

Key principle: **Scoring and solving are separated.** The `Solver` trait receives
pre-scored objects and doesn't know about economic parameters.

## Testing

- Unit tests: alongside source code or in `tests/` directories
- Property-based tests: use `proptest` for solver invariants
- Acceptance tests: `#[ignore]` attribute, run with `cargo test --release -- --ignored`

## Reporting Issues

- Use GitHub Issues
- Include: expected behavior, actual behavior, steps to reproduce
- For performance issues: include trace size, capacity, and preset used
