# cargo-quickclean

# Installation

```sh
cargo install --git https://github.com/probably-neb/cargo-quickclean
```

# Usage

```sh
cargo quickclean ~/code/*
```

Targets are normally renamed out of the way before deletion. If preparing or
renaming a target fails because the disk is full or a storage quota is exhausted,
quickclean deletes that target in place instead.
Avoid running builds during cleanup: in-place deletion can race with them.
