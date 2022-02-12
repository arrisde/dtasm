// Copyright 2021 Siemens AG
// SPDX-License-Identifier: MIT

#![no_std]
#![feature(core_intrinsics, lang_items, default_alloc_error_handler)]

extern crate alloc;
extern crate wee_alloc;
extern crate compiler_builtins;

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[panic_handler]
#[no_mangle]
fn panic_handler(_info: &core::panic::PanicInfo) -> ! {
    ::core::intrinsics::abort();
    //loop {}
}

// // Need to provide an allocation error handler which just aborts
// // the execution with trap.
// #[alloc_error_handler]
// #[no_mangle]
// pub extern "C" fn oom(_: ::core::alloc::Layout) -> ! {
//     unsafe {
//         ::core::intrinsics::abort();
//     }
// }

pub fn set_panic_hook() {
    // When the `console_error_panic_hook` feature is enabled, we can call the
    // `set_panic_hook` function at least once during initialization, and then
    // we will get better error messages if our code ever panics.
    //
    // For more details see
    // https://github.com/rustwasm/console_error_panic_hook#readme
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}


mod add_types;
mod interface;
mod adder;
mod errors;
mod dtasm;