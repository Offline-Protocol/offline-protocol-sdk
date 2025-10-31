# Contributing to Offline Protocol SDK

Thank you for your interest in contributing! This document provides guidelines for contributing to the project.

## Development Setup

### Prerequisites

- Rust 1.70+ (`rustup default stable`)
- For mobile: Android NDK, Xcode
- For web: wasm-pack

### Clone and Build

```bash
git clone https://github.com/offline-protocol/sdk
cd offline-protocol-sdk
cargo build --all
cargo test --all
```

## Code Quality Standards

### Before Every Commit

Run these checks (they must all pass):

```bash
# 1. Build without errors
cargo build --all

# 2. All tests pass
cargo test --all

# 3. No clippy warnings
cargo clippy --all -- -D warnings

# 4. Code is formatted
cargo fmt --all
```

### Safety Requirements

- **Core crates**: `#![deny(unsafe_code)]` - zero unsafe code allowed
- **FFI crate**: Unsafe code is permitted but must:
  - Have SAFETY comments explaining why it's safe
  - Validate all pointers (null checks)
  - Catch all panics (`catch_unwind`)
  - Be reviewed by maintainers

### Testing Requirements

- New features must include tests
- Bug fixes must include regression tests
- Aim for >80% code coverage
- Integration tests for end-to-end scenarios

## Commit Message Format

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>

[optional body]
```

**Types**:
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation
- `test`: Adding tests
- `refactor`: Code refactoring
- `perf`: Performance improvement
- `chore`: Build/tooling changes

**Scopes**:
- `core`: offline-protocol-core
- `transport`: offline-protocol-transport
- `router`: offline-protocol-router (DORS)
- `reliability`: offline-protocol-reliability
- `protocol`: offline-protocol (main API)
- `ffi`: offline-protocol-ffi
- `bindings`: Platform bindings

**Examples**:
```
feat(router): add congestion-aware path selection
fix(reliability): correct retry backoff calculation
docs(api): update configuration reference
test(dors): add tests for transport switching
```

## Code Organization

### Adding a New Feature

1. **Core Logic**: Implement in appropriate Rust crate (100% safe)
2. **Tests**: Add comprehensive unit tests
3. **FFI**: Expose via C FFI if needed for platforms
4. **Bindings**: Update platform bindings (React Native, etc.)
5. **Docs**: Update README and relevant docs
6. **Commit**: Use conventional commits format

### File Structure

```
offline-protocol-sdk/
├── crates/          # Rust crates (core logic)
├── bindings/        # Platform bindings
├── docs/            # Documentation
├── examples/        # Example applications
└── tests/           # Integration tests
```

## Pull Request Process

1. Fork the repository
2. Create a feature branch (`git checkout -b feat/my-feature`)
3. Make your changes
4. Run quality checks (build, test, clippy, fmt)
5. Commit with conventional commits
6. Push to your fork
7. Create a Pull Request

### PR Checklist

- [ ] Tests pass (`cargo test --all`)
- [ ] No clippy warnings (`cargo clippy --all -- -D warnings`)
- [ ] Code formatted (`cargo fmt --all`)
- [ ] Documentation updated
- [ ] Conventional commit messages
- [ ] No breaking changes (or clearly documented)

## Development Workflow

### Running Tests

```bash
# All tests
cargo test --all

# Specific crate
cargo test --package offline-protocol-core

# Specific test
cargo test test_message_creation

# With output
cargo test -- --nocapture
```

### Linting

```bash
# Check for issues
cargo clippy --all

# Fix automatically fixable issues
cargo clippy --all --fix

# Strict mode (required for CI)
cargo clippy --all -- -D warnings
```

### Formatting

```bash
# Format all code
cargo fmt --all

# Check formatting without applying
cargo fmt --all -- --check
```

### Documentation

```bash
# Generate and open docs
cargo doc --all --open

# Check doc comments
cargo doc --all --no-deps
```

## Architecture Decisions

When making significant changes:

1. **Safety First**: Prefer safe Rust over unsafe
2. **Performance**: Measure before optimizing
3. **Simplicity**: Clear code over clever code
4. **Testing**: Test coverage for all paths
5. **Documentation**: Public APIs must be documented

## Questions?

- Open a [Discussion](https://github.com/offline-protocol/sdk/discussions)
- Ask in [Issues](https://github.com/offline-protocol/sdk/issues)

## License

By contributing, you agree that your contributions will be licensed under MIT OR Apache-2.0.

