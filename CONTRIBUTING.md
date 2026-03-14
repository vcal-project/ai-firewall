# Contributing to AI Cost Firewall

Thank you for your interest in contributing.

AI Cost Firewall is an early-stage open source project and contributions are welcome.

## Ways to contribute

- report bugs
- suggest new features
- improve documentation
- add tests
- improve performance

## Development setup

Clone the repository:

```bash
git clone https://github.com/vcal-project/ai-firewall.git
cd ai-firewall
```

Build the project:

```bash
cargo build
```

Run tests:

```bash
cargo test
```

## Pull Requests

Please follow these guidelines:

- open an issue before submitting major changes
- keep pull requests focused
- include documentation updates if necessary

## Code style

The project follows standard Rust conventions.

Run:

```bash
cargo fmt
cargo clippy
```

before submitting a pull request.


Clippy warnings are treated seriously in this project. The codebase enables additional lints to prevent common production issues such as:

- accidental `unwrap()` or `expect()` usage
- panics in request-handling paths
- indexing that may panic
- `todo!()` or `unimplemented!()` left in production code
- holding locks across `.await`

Please ensure your changes pass `cargo clippy` without warnings.

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
