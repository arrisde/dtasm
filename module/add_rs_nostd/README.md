## Rust _dtasm_ module using _no_std_ [WIP]
This is a _dtasm_ example module in __no_std__ Rust. Purpose of this implementation is to find out by how much the size of a Rust _dtasm_ module could be reduced by not including std lib. Main challenges are that _flatbuffers_ and _thiserror_ dependencies of [_dtasm_base_](../../lib/dtasm_base_rs) do not support __no_std__ currently, which has been worked around in this implementation. 

These are the current outcomes:
```
> cd ../add_rs && cargo build --release && stat -c "%s" target/wasm32-wasi/release/add_rs.wasm && cd -
474757

> cargo build --release && stat -c "%s" target/wasm32-wasi/release/add_rs_nostd.wasm
181058
```

Using `wasm-opt` from [Binaryen](http://webassembly.github.io/binaryen/) and `wasm-strip` from [WABT](https://github.com/WebAssembly/wabt), the size can be further reduced:
```
> wasm-strip target/wasm32-wasi/release/add_rs_nostd.wasm
> wasm-opt -Oz -o add_rs_nostd_opt.wasm target/wasm32-wasi/release/add_rs_nostd.wasm
> stat -c "%s" add_rs_nostd_opt.wasm
74526
```
