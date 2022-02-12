// Copyright 2021 Siemens AG
// SPDX-License-Identifier: MIT

extern crate alloc;

use alloc::slice;
use alloc::vec;
pub use dtasm_base::{types,model_description};

use dtasm_abi::generated::dtasm_api as DTAPI;
use dtasm_abi::generated::dtasm_types as DTT;
use dtasm_abi::generated::dtasm_model_description as DTMD;
use flatbuffers as FB;

use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::alloc::{alloc as my_alloc, dealloc as my_dealloc, Layout};

use once_cell::unsync::Lazy;

use dtasm_base::model_conversion::{convert_model_description,collect_var_types};
use dtasm_base::types::{DtasmVarType,DtasmVarValues};

use crate::interface;

static mut SIM_MODULE: Lazy<Box<dyn interface::DtasmIf + Sync + Send>> = Lazy::new(|| Box::new(crate::adder::AddMod));
static mut VARTYPES: Option<BTreeMap<i32, DtasmVarType>> = None;
static mut MDBYTES: Option<&[u8]> = Some(&[]);
static mut FBBUILDER: Option<FB::FlatBufferBuilder> = None;

#[no_mangle]
extern "C" fn alloc(len: usize) -> *mut u8 {
    let align = core::mem::align_of::<usize>();
    let layout = unsafe { Layout::from_size_align_unchecked(len, align) };
    unsafe { my_alloc(layout) }
}

#[no_mangle]
extern "C" fn dealloc(ptr: *mut u8, size: usize) {
    let align = core::mem::align_of::<usize>();
    let layout = unsafe { Layout::from_size_align_unchecked(size, align) };
    unsafe { my_dealloc(ptr, layout) };
}

#[no_mangle]
extern "C" fn getModelDescription(out_p: *mut u8, max_len: u32) -> u32 {
    let bytes = unsafe { slice::from_raw_parts_mut(out_p, max_len as usize) };

    let mut md_bytes  = unsafe { MDBYTES.unwrap() };

    if md_bytes.len() == 0 {
        md_bytes = unsafe { SIM_MODULE.get_model_description().unwrap() };

        let md_dtasm = unsafe { DTMD::root_as_model_description_unchecked(md_bytes) };
        let md = convert_model_description(&md_dtasm);

        let var_types = collect_var_types(&md);
        unsafe { VARTYPES = Some(var_types) }
    }

    if md_bytes.len() > max_len as usize {
        return md_bytes.len() as u32;
    }
    else
    {
        bytes[..md_bytes.len()].copy_from_slice(md_bytes);
    }
    unsafe { MDBYTES = Some(md_bytes); }

    return md_bytes.len() as u32;
}

#[no_mangle]
extern "C" fn init(in_p: *const u8, in_len: u32, out_p: *mut u8, out_max_len: u32) -> u32
{
    let in_bytes = unsafe { slice::from_raw_parts(in_p, in_len as usize) };
    let init_req = unsafe { FB::root_unchecked::<DTAPI::InitReq>(in_bytes) };

    let mut init_vals_sim = DtasmVarValues::new();

    let init_vals = init_req.init_values().unwrap();
    init_vals.bool_vals().map(|bools| for boolean in bools.iter() {
        let id = boolean.id();
        let val = boolean.val();
        init_vals_sim.bool_values.insert(id, val);
    });
    init_vals.real_vals().map(|reals| for real in reals.iter() {
        let id = real.id();
        let val = real.val();
        init_vals_sim.real_values.insert(id, val);
    });
    init_vals.int_vals().map(|integers| for integer in integers.iter() {
        let id = integer.id();
        let val = integer.val();
        init_vals_sim.int_values.insert(id, val);
    });

    let md_bytes = unsafe { MDBYTES.unwrap() };
    let md_dtasm = DTMD::root_as_model_description(md_bytes).unwrap();
    let md = convert_model_description(&md_dtasm);
    
    let init_res =
    unsafe { 
         SIM_MODULE.initialize(&md,
             &init_vals_sim, 
            init_req.starttime(), 
            match init_req.endtime_set() {
                true => Some(init_req.endtime()),
                false => None
            }, 
            match init_req.tolerance_set() {
                true => Some(init_req.tolerance()),
                false => None
            },
            init_req.loglevel_limit().into(), 
            init_req.check_consistency()
        )
    };

    let ret_val: u32;
    unsafe { FBBUILDER = Some(FB::FlatBufferBuilder::with_capacity(4096)) };
    let mut fb_builder = unsafe { FBBUILDER.as_mut().unwrap() };
    {
        let status_res = DTAPI::StatusRes::create(&mut fb_builder, &DTAPI::StatusResArgs{
            status: match init_res {
                Err(_err) => DTT::Status::Error, 
                Ok(status) => status.into()
            }
        });

        fb_builder.finish(status_res, None);
        let buf = fb_builder.finished_data(); 

        let bytes = unsafe { slice::from_raw_parts_mut(out_p, out_max_len as usize) };

        if buf.len() <= out_max_len as usize {
            bytes[..buf.len()].copy_from_slice(buf);
        }

        ret_val = buf.len() as u32;
    }

    fb_builder.reset();
    ret_val
}

#[no_mangle]
extern "C" fn getValues(in_p: *const u8, in_len: u32, out_p: *mut u8, out_max_len: u32) -> u32
{    
    let in_bytes = unsafe { slice::from_raw_parts(in_p, in_len as usize) };
    let getvalues_req = unsafe { FB::root_unchecked::<DTAPI::GetValuesReq>(in_bytes) };

    let get_ids = getvalues_req.ids().expect("Get values request did not contain any variables.");
    let mut get_var_ids: Vec<i32> = vec!();

    for i in 0..get_ids.len() {
        get_var_ids.push(get_ids.get(i));
    }

    let get_values_res =
        unsafe { 
            SIM_MODULE.get_values(&get_var_ids).expect("Error on calling get_values: ")
        };

    let ret_val: u32;
    let mut fb_builder = unsafe { FBBUILDER.as_mut().unwrap() };
    {
        let mut real_offs: Vec<flatbuffers::WIPOffset<DTT::RealVal>> = Vec::new();
        let real_vals = get_values_res.values.real_values;
        for (key, value) in real_vals {
            let real_val = DTT::RealVal::create(&mut fb_builder, &DTT::RealValArgs{
                id: key,
                val: value
            });
            real_offs.push(real_val);
        }
        let real_vals_fb = fb_builder.create_vector(&real_offs);

        let mut int_offs: Vec<flatbuffers::WIPOffset<DTT::IntVal>> = Vec::new();
        let int_vals = get_values_res.values.int_values;
        for (key, value) in int_vals {
            let int_val = DTT::IntVal::create(&mut fb_builder, &DTT::IntValArgs{
                id: key,
                val: value
            });
            int_offs.push(int_val);
        }
        let int_vals_fb = fb_builder.create_vector(&int_offs);

        let mut bool_offs: Vec<flatbuffers::WIPOffset<DTT::BoolVal>> = Vec::new();
        let bool_vals = get_values_res.values.bool_values;
        for (key, value) in bool_vals {
            let bool_val = DTT::BoolVal::create(&mut fb_builder, &DTT::BoolValArgs{
                id: key,
                val: value
            });
            bool_offs.push(bool_val);
        }
        let bool_vals_fb = fb_builder.create_vector(&bool_offs);

        let scalar_vals = DTT::VarValues::create(&mut fb_builder, &DTT::VarValuesArgs{
            real_vals: Some(real_vals_fb), 
            int_vals: Some(int_vals_fb),
            bool_vals: Some(bool_vals_fb),
            string_vals: None
        });

        let get_values_res_fb = DTAPI::GetValuesRes::create(&mut fb_builder, &DTAPI::GetValuesResArgs{
            current_time: get_values_res.current_time,
            values: Some(scalar_vals), 
            status: get_values_res.status.into()
        });

        fb_builder.finish(get_values_res_fb, None);
        let buf = fb_builder.finished_data(); 

        let bytes = unsafe { slice::from_raw_parts_mut(out_p, out_max_len as usize) };

        if buf.len() <= out_max_len as usize {
            bytes[..buf.len()].copy_from_slice(buf);
        }

        ret_val = buf.len() as u32;
    }

    fb_builder.reset();
    ret_val
}

#[no_mangle]
extern "C" fn setValues(in_p: *const u8, in_len: u32, out_p: *mut u8, out_max_len: u32) -> u32
{
    let in_bytes = unsafe { slice::from_raw_parts(in_p, in_len as usize) };
    let set_req = unsafe { FB::root_unchecked::<DTAPI::SetValuesReq>(in_bytes) };

    let mut set_vals_sim = DtasmVarValues::new();

    let set_vals = set_req.values().unwrap();
    set_vals.bool_vals().map(|bools| for boolean in bools.iter() {
        let id = boolean.id();
        let val = boolean.val();
        set_vals_sim.bool_values.insert(id, val);
    });
    set_vals.real_vals().map(|reals| for real in reals.iter() {
        let id = real.id();
        let val = real.val();
        set_vals_sim.real_values.insert(id, val);
    });
    set_vals.int_vals().map(|integers| for integer in integers.iter() {
        let id = integer.id();
        let val = integer.val();
        set_vals_sim.int_values.insert(id, val);
    });

    let set_vals_res =
    unsafe { 
         SIM_MODULE.set_values(&set_vals_sim)
    };

    let ret_val: u32;
    let mut fb_builder = unsafe { FBBUILDER.as_mut().unwrap() };
    {
        let status_res = DTAPI::StatusRes::create(&mut fb_builder, &DTAPI::StatusResArgs{
            status: match set_vals_res {
                Err(_err) => DTT::Status::Error, 
                Ok(status) => status.into()
            }
        });

        fb_builder.finish(status_res, None);
        let buf = fb_builder.finished_data(); 

        let bytes = unsafe { slice::from_raw_parts_mut(out_p, out_max_len as usize) };

        if buf.len() <= out_max_len as usize {
            bytes[..buf.len()].copy_from_slice(buf);
        }

        ret_val = buf.len() as u32;
    }

    fb_builder.reset();
    ret_val
}

#[no_mangle]
extern "C" fn doStep(in_p: *const u8, in_len: u32, out_p: *mut u8, out_max_len: u32) -> u32
{
    let in_bytes = unsafe { slice::from_raw_parts(in_p, in_len as usize) };
    let dostep_req = unsafe { FB::root_unchecked::<DTAPI::DoStepReq>(in_bytes) };

    let current_time = dostep_req.current_time();
    let step = dostep_req.timestep();

    let do_step_res =
    unsafe { 
        SIM_MODULE.do_step(current_time, step).expect("Error on calling do_step: ")
    };

    let ret_val: u32;
    let mut fb_builder = unsafe { FBBUILDER.as_mut().unwrap() };
    {
        let do_step_res_fb = DTAPI::DoStepRes::create(&mut fb_builder, &DTAPI::DoStepResArgs{
            status: do_step_res.status.into(),
            updated_time: do_step_res.updated_time
        });

        fb_builder.finish(do_step_res_fb, None);
        let buf = fb_builder.finished_data(); 

        let bytes = unsafe { slice::from_raw_parts_mut(out_p, out_max_len as usize) };

        if buf.len() <= out_max_len as usize {
            bytes[..buf.len()].copy_from_slice(buf);
        }

        ret_val = buf.len() as u32;
    }

    fb_builder.reset();
    ret_val
}
