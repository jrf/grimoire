set shell := ["bash", "-uc"]

default:
    @just --list

build:
    cargo build

release:
    cargo build --release

run *args:
    cargo run -- {{ args }}

check:
    cargo clippy
    cargo fmt --check

check-all:
    ./scripts/check-all

fmt:
    cargo fmt

clean:
    cargo clean

build-extension:
    ./scripts/build-extension

install:
    cargo install --path . --locked --force
    @if [ "$(uname)" = "Darwin" ]; then codesign -s - ~/.cargo/bin/grimoire; fi
    @echo "Installed → ~/.cargo/bin/grimoire"
