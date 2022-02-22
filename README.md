# SSDSync

**WARNING: this is experimental software. It can very
well destroy all your data.**

SSDSync tries to copy data between files or block devices
so it uses as few writes as possible. This is to protect
SSDs from premature wearing.

SSDSync aims to replace simple "dd" style data transfers
solutions. It reads both source and target, and compares
blocks. Only differing blocks are written to the target.

## Building

SSDSync is written in rust, and can be build with cargo:

```
cargo build
```

## Install

```
cargo install .
```
