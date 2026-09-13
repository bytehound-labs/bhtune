# CLA signature store

This branch exists only to store Contributor License Agreement signatures. It deliberately
contains no source code and is not merged into `main`.

Signatures are appended to `signatures/version1/cla.json` by
[`.github/workflows/cla.yml`](https://github.com/bytehound-labs/bhtune/blob/main/.github/workflows/cla.yml)
when a contributor signs on their pull request. Signatures live here rather than on `main`
because `main` is protected and requires pull requests, so an automated push to it is rejected.

The agreement itself is [`CLA.md`](https://github.com/bytehound-labs/bhtune/blob/main/CLA.md).

Do not edit this branch by hand.
