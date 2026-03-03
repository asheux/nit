set shell := ["bash", "-cu"]

default:
    just fmt

fmt:
    cargo fmt

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --all-targets --all-features -- -D warnings

test:
    cargo test --all

deny:
    cargo deny check

ci:
    just fmt-check
    just clippy
    just test
    just deny

run *args:
    cargo run -- {{args}}
