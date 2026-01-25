set shell := ["bash", "-cu"]

default:
    just fmt

fmt:
    cargo fmt

clippy:
    cargo clippy --all-targets --all-features -D warnings

test:
    cargo test --all

deny:
    cargo deny check

ci:
    just fmt
    just clippy
    just test
    just deny

run *args:
    cargo run -- {{args}}

