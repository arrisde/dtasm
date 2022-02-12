// Copyright 2021 Siemens AG
// SPDX-License-Identifier: MIT

use std::convert::identity; 
use std::error::Error;
use std::path::PathBuf;
use std::io::{Read, Write};
use std::collections::HashMap;

use flatbuffers as FB;
use wasmtime as WT;
use wasmtime_wasi as WTW;

use dtasm_abi::generated::dtasm_api as DTAPI;
use dtasm_abi::generated::dtasm_types as DTT;
use dtasm_abi::generated::dtasm_model_description as DTMD;

use crate::errors::DtasmtimeError;
use DtasmtimeError::DtasmError as DTERR; 
use dtasm_base::model_conversion::convert_model_description;
use dtasm_base::model_description as MD;
use dtasm_base::types::{DtasmVarType,DtasmVarValues,LogLevel,Status,GetValuesResponse,DoStepResponse};
use dtasm_base::errors::DtasmError;

type In1Out1T = dyn Fn(i32,) -> Result<i32, WT::Trap>;
type In2Out0T = dyn Fn(i32, i32) -> Result<(), WT::Trap>;
type In2Out1T = dyn Fn(i32, i32) -> Result<i32, WT::Trap>;
type In4Out1T = dyn Fn(i32, i32, i32, i32) -> Result<i32, WT::Trap>;

const WASM_PAGE_SIZE: u32 = 65536;
const FB_BUILDER_SIZE: usize = 32768;
const BASE_MEM_SIZE: i32 = 2048;

/// dtasm interface functions
static DTASM_EXPORTS: [&str; 8] = [
    "memory", 
    "alloc", 
    "dealloc", 
    "getModelDescription", 
    "init", 
    "getValues", 
    "setValues",
    "doStep"];

/// Engine for executing modules
pub struct Engine {
    wt_store: WT::Store, 
    wt_linker: WT::Linker,
}

impl Engine {
    pub fn new() -> Result<Engine, Box<dyn Error>> {
        let store = WT::Store::default();
        let mut linker = WT::Linker::new(&store);

        let wasi = WTW::Wasi::new(&store, WTW::WasiCtx::new(std::env::args())?);
        wasi.add_to_linker(&mut linker)?;

        Ok(Engine {
            wt_store: store, 
            wt_linker: linker, 
        })
    }
}

/// Represents a dtasm module in memory
pub struct Module<'a> {
    wt_module: WT::Module,
    dtasm_engine: &'a Engine 
}

impl Module<'_> {
    /// Loads a module from bytestream; note that the module needs to be tied to an engine at this point
    pub fn new(file: PathBuf, engine: &Engine) -> Result<Module, DtasmtimeError> {
        let store = &engine.wt_store;
        let module = WT::Module::from_file(store.engine(), file)?;

        for name in DTASM_EXPORTS.iter() {
            if module.get_export(name).is_none() {
                return Err(DTERR(DtasmError::MissingDtasmExport(name.to_string())));
            }
        }

        // TODO: ensure that exports have expected signature

        Ok(Module {
            wt_module: module, 
            dtasm_engine: engine
        })
    }

    /// Create an instance of the module
    pub fn instantiate(&self) -> Result<Instance, DtasmtimeError> {
        let wt_instance = self.dtasm_engine.wt_linker.instantiate(&self.wt_module)?;

        let reactor_init = wt_instance
            .get_func("_initialize");
        let memory = wt_instance
            .get_memory("memory")
            .ok_or(DTERR(DtasmError::MissingDtasmExport("memory".to_string())))?;
        let alloc = wt_instance
            .get_func("alloc")
            .ok_or(DTERR(DtasmError::MissingDtasmExport("alloc".to_string())))?
            .get1::<i32, i32>()?;
        let dealloc = wt_instance
            .get_func("dealloc")
            .ok_or(DTERR(DtasmError::MissingDtasmExport("dealloc".to_string())))?
            .get2::<i32, i32, ()>()?;
        let get_model_description = wt_instance
            .get_func("getModelDescription")
            .ok_or(DTERR(DtasmError::MissingDtasmExport("getModelDescription".to_string())))?
            .get2::<i32, i32, i32>()?;
        let init = wt_instance
            .get_func("init")
            .ok_or(DTERR(DtasmError::MissingDtasmExport("init".to_string())))?
            .get4::<i32, i32, i32, i32, i32>()?;
        let get_values = wt_instance
            .get_func("getValues")
            .ok_or(DTERR(DtasmError::MissingDtasmExport("getValues".to_string())))?
            .get4::<i32, i32, i32, i32, i32>()?;
        let set_values = wt_instance
            .get_func("setValues")
            .ok_or(DTERR(DtasmError::MissingDtasmExport("setValues".to_string())))?
            .get4::<i32, i32, i32, i32, i32>()?;
        let do_step = wt_instance
            .get_func("doStep")
            .ok_or(DTERR(DtasmError::MissingDtasmExport("doStep".to_string())))?
            .get4::<i32, i32, i32, i32, i32>()?;

        Ok(Instance {
            memory, 
            reactor_init_fn: reactor_init,
            alloc_fn: Box::new(alloc), 
            dealloc_fn: Box::new(dealloc), 
            get_md_fn: Box::new(get_model_description), 
            init_fn: Box::new(init),
            get_values_fn: Box::new(get_values),
            set_values_fn: Box::new(set_values),
            do_step_fn: Box::new(do_step),
            var_types: HashMap::new(),
            md: None, 
            builder: FB::FlatBufferBuilder::new_with_capacity(FB_BUILDER_SIZE)
        })
    }
}

/// Represents an instance of a loaded dtasm module
pub struct Instance {
    memory: WT::Memory, 
    reactor_init_fn: Option<WT::Func>,
    alloc_fn: Box<In1Out1T>, 
    dealloc_fn: Box<In2Out0T>, 
    get_md_fn: Box<In2Out1T>, 
    init_fn: Box<In4Out1T>,
    get_values_fn: Box<In4Out1T>,
    do_step_fn: Box<In4Out1T>,
    set_values_fn: Box<In4Out1T>,
    var_types: HashMap<i32, DtasmVarType>,
    md: Option<MD::ModelDescription>, 
    builder: FB::FlatBufferBuilder<'static>
}

impl Instance {
    /// Retrieve the model description of this module by calling the `getModelDescription` 
    /// export
    pub fn get_model_description(&mut self) -> Result<MD::ModelDescription, DtasmtimeError> {

        // if model description was already loaded, return it from cache
        match &self.md {
            None => {}, 
            Some(mod_desc) => {
                return Ok(mod_desc.clone());
            }
        } 

        let mut size = BASE_MEM_SIZE;
        let mut mem = (*self.alloc_fn)(size)?;
        let mut size_out = (*self.get_md_fn)(mem, size)?;

        while size_out > size {
            (*self.dealloc_fn)(mem, size)?;
            size *= 2;
            mem = (*self.alloc_fn)(size)?;

            size_out = (*self.get_md_fn)(mem, size)?;
        }

        let bytes = unsafe {
            &self.memory.data_unchecked()[mem as usize..(mem+size_out) as usize] 
        };
   
        let model_desc_fb = DTMD::get_root_as_model_description(bytes);
        let md = convert_model_description(&model_desc_fb);
        self.md = Some(md.clone());
        self.var_types = Instance::collect_var_types(&md)?;

        (*self.dealloc_fn)(mem, size)?;
   
        Ok(md)
    }

    /// Initialize the instance with the given initial values and simulation parameters
    ///
    /// * `initial_vals` - initial values for the state variables
    /// * `tmin` - initial time where simulation starts
    /// * `tmax` - final time of the simulation
    /// * `tol` - relative tolerance for numerical solver
    /// * `log_level` - maximal level at which log messages should be reported
    /// * `check` - whether to check validity of buffers (not currently implemented)
    pub fn initialize(&mut self, initial_vals: &DtasmVarValues, tmin: f64, tmax: Option<f64>, 
        tol: Option<f64>, log_level: LogLevel, check: bool) -> Result<Status, DtasmtimeError>{
        // TODO: Check if state valid

        let md = &self.md.as_ref().ok_or(DTERR(DtasmError::InvalidCallingOrder))?;
        
        // if _initialize is exported, call it now to initialize WASI reactor
        match &self.reactor_init_fn {
            None => (),
            Some(f) => {
                let init_fn = f.get0::<()>()?;
                init_fn()?
            }
        }
        
        let fb_log = match log_level {
            LogLevel::Info => DTT::LogLevel::Info,
            LogLevel::Warn => DTT::LogLevel::Warn,
            LogLevel::Error => DTT::LogLevel::Error
        };

        let mut var_values = DtasmVarValues::new();

        // collect all initial values that are explicitly set and check their types
        for (id, val) in &initial_vals.real_values {
            if !self.var_types.contains_key(id) { 
                return Err(DTERR(DtasmError::UnknownVariableId(*id))); 
            }
            if self.var_types[id].value_type != MD::VariableType::DtasmReal { 
                return Err(DTERR(DtasmError::VariableTypeMismatch(self.var_types[id].value_type, *id))); 
            }
            var_values.real_values.insert(*id, *val);
        }
        for (id, val) in &initial_vals.int_values {
            if !self.var_types.contains_key(id) { 
                return Err(DTERR(DtasmError::UnknownVariableId(*id))); 
            }
            if self.var_types[id].value_type != MD::VariableType::DtasmInt { 
                return Err(DTERR(DtasmError::VariableTypeMismatch(self.var_types[id].value_type, *id))); 
            }
            var_values.int_values.insert(*id, *val);
        }
        for (id, val) in &initial_vals.bool_values {
            if !self.var_types.contains_key(id) { 
                return Err(DTERR(DtasmError::UnknownVariableId(*id))); 
            }
            if self.var_types[id].value_type != MD::VariableType::DtasmBool { 
                return Err(DTERR(DtasmError::VariableTypeMismatch(self.var_types[id].value_type, *id))); 
            }
            var_values.bool_values.insert(*id, *val);
        }
        for (id, val) in &initial_vals.string_values {
            if !self.var_types.contains_key(id) { 
                return Err(DTERR(DtasmError::UnknownVariableId(*id))); 
            }
            if self.var_types[id].value_type != MD::VariableType::DtasmString { 
                return Err(DTERR(DtasmError::VariableTypeMismatch(self.var_types[id].value_type, *id))); 
            }
            var_values.string_values.insert(*id, val.clone());
        }

        // build up the init request message
        let model_id = self.builder.create_string(&md.model.id);

        let mut real_offs: Vec<flatbuffers::WIPOffset<DTT::RealVal>> = Vec::new();
        for (id, val) in &var_values.real_values {
            real_offs.push(DTT::RealVal::create(&mut self.builder, &DTT::RealValArgs{
                id: *id,
                val: *val
            }));
        }
        let real_vals = self.builder.create_vector(&real_offs);

        let mut int_offs: Vec<flatbuffers::WIPOffset<DTT::IntVal>> = Vec::new();
        for (id, val) in &var_values.int_values {
            int_offs.push(DTT::IntVal::create(&mut self.builder, &DTT::IntValArgs{
                id: *id,
                val: *val
            }));
        }
        let int_vals = self.builder.create_vector(&int_offs);

        let mut bool_offs: Vec<flatbuffers::WIPOffset<DTT::BoolVal>> = Vec::new();
        for (id, val) in &var_values.bool_values {
            bool_offs.push(DTT::BoolVal::create(&mut self.builder, &DTT::BoolValArgs{
                id: *id,
                val: *val
            }));
        }
        let bool_vals = self.builder.create_vector(&bool_offs);

        let mut string_offs: Vec<flatbuffers::WIPOffset<DTT::StringVal>> = Vec::new();
        for (id, val) in &var_values.string_values {
            let val_str = self.builder.create_string(val);
            string_offs.push(DTT::StringVal::create(&mut self.builder, &DTT::StringValArgs{
                id: *id,
                val: Some(val_str)
            }));
        }
        let string_vals = self.builder.create_vector(&string_offs);

        let scalar_vals = DTT::VarValues::create(&mut self.builder, &DTT::VarValuesArgs{
            real_vals: Some(real_vals), 
            int_vals: Some(int_vals),
            bool_vals: Some(bool_vals),
            string_vals: Some(string_vals)
        });

        let init_req = DTAPI::InitReq::create(&mut self.builder, &DTAPI::InitReqArgs{
            id: Some(model_id), 
            starttime: tmin, 
            endtime: match tmax {Some(v) => v, None => 0.0}, 
            endtime_set: match tmax {Some(_v) => true, None => false}, 
            tolerance: match tol {Some(v) => v, None => 0.0}, 
            tolerance_set: match tol {Some(_v) => true, None => false}, 
            loglevel_limit: fb_log, 
            check_consistency: check, 
            init_values: Some(scalar_vals)
        });
        self.builder.finish(init_req, None);

        let init_req_buf = self.builder.finished_data(); 
        let init_req_len = init_req_buf.len();
        let init_req_ptr = (*self.alloc_fn)(init_req_len as i32)? as usize;

        // copy buffer into allocated position in linear memory
        unsafe {
            self.memory.data_unchecked_mut()[init_req_ptr..init_req_ptr+init_req_len]
                .copy_from_slice(init_req_buf);
        };

        // return value is status only, should fit into 64 bytes
        let size = 64;
        let init_res_ptr = (*self.alloc_fn)(size)? as usize;
        let size_out = (self.init_fn)(init_req_ptr as i32, init_req_len as i32, init_res_ptr as i32, size)?;

        if size_out > size { return Err(DTERR(DtasmError::DtasmInternalError(format!("Unexpected size returned from init request: {}", size_out)))); }

        let res_bytes = unsafe {
            &self.memory.data_unchecked()[init_res_ptr..init_res_ptr+(size_out as usize)] 
        };

        let init_res = unsafe { FB::root_unchecked::<DTAPI::StatusRes>(res_bytes) };

        let status_res = init_res.status().into();
        
        (*self.dealloc_fn)(init_req_ptr as i32, init_req_len as i32)?;
        (*self.dealloc_fn)(init_res_ptr as i32, size)?;
        self.builder.reset();

        Ok(status_res)
    }

    /// Retrieve values of the output and state variables in the current timestep. 
    /// 
    /// * `var_ids` - vector of variable ids for which values shall be retrieved
    pub fn get_values(&mut self, var_ids: &Vec<i32>) -> Result<GetValuesResponse, DtasmtimeError> {
        // TODO: Check state

        // check if all requested var ids are valid
        for id in var_ids.iter() {
            if !self.var_types.contains_key(id) { 
                return Err(DTERR(DtasmError::UnknownVariableId(*id))); 
            }
            if self.var_types[id].causality == MD::CausalityType::Input {
                return Err(DTERR(DtasmError::VariableCausalityMismatch(MD::CausalityType::Input,*id))); 
            }
        }

        // build get_values request message
        let var_ids_fb = self.builder.create_vector(&var_ids);

        let req = DTAPI::GetValuesReq::create(&mut self.builder, &DTAPI::GetValuesReqArgs{
            ids: Some(var_ids_fb)
        });
        self.builder.finish(req, None);

        let getval_req_buf = self.builder.finished_data();
        let getval_req_len = getval_req_buf.len();
        let getval_req_ptr = (*self.alloc_fn)(getval_req_len as i32)? as usize;

        unsafe {
            self.memory.data_unchecked_mut()[getval_req_ptr..getval_req_ptr+getval_req_len]
                .copy_from_slice(getval_req_buf);
           };
    
        let mut size = BASE_MEM_SIZE;
        let mut getval_res_ptr = (*self.alloc_fn)(size)? as usize;
        let mut size_out = (*self.get_values_fn)(getval_req_ptr as i32, getval_req_len as i32, getval_res_ptr as i32, size)?;
        
        while size_out > size {
            (*self.dealloc_fn)(getval_res_ptr as i32, size)?;
            size *= 2;
            getval_res_ptr = (*self.alloc_fn)(size)? as usize;
    
            size_out = (*self.get_values_fn)(getval_req_ptr as i32, getval_req_len as i32, getval_res_ptr as i32, size)?;
        }
    
        let res_bytes = unsafe {
            &self.memory.data_unchecked()[getval_res_ptr..getval_res_ptr+size_out as usize]
        };
    
        let getvalues_res = unsafe { FB::root_unchecked::<DTAPI::GetValuesRes>(res_bytes) };
        let var_values = Instance::extract_vals(&getvalues_res, &self.var_types)?;
        let current_time = getvalues_res.current_time();
        let status = getvalues_res.status().into();

        (*self.dealloc_fn)(getval_req_ptr as i32, getval_req_len as i32)?;
        (*self.dealloc_fn)(getval_res_ptr as i32, size)?;
        self.builder.reset();

        Ok(GetValuesResponse {status, current_time, values: var_values})
    }


    /// Set values of input variables for the next timestep
    ///
    /// * `input_vals`: Values for the input variables
    pub fn set_values(&mut self, input_vals: &DtasmVarValues) -> Result<Status, DtasmtimeError>{
        // TODO: check state

        // start with default values from model description
        let mut var_values = DtasmVarValues::new();

        // collect set values and check their existence and types
        for (id, val) in &input_vals.real_values {
            if !self.var_types.contains_key(id) { 
                return Err(DTERR(DtasmError::UnknownVariableId(*id))); 
            }
            if self.var_types[id].causality != MD::CausalityType::Input { 
                return Err(DTERR(DtasmError::VariableCausalityInvalidForSet(self.var_types[id].causality, *id))); 
            }
            if self.var_types[id].value_type != MD::VariableType::DtasmReal { 
                return Err(DTERR(DtasmError::VariableTypeMismatch(self.var_types[id].value_type, *id))); 
            }
            var_values.real_values.insert(*id, *val);
        }
        for (id, val) in &input_vals.int_values {
            if !self.var_types.contains_key(id) { 
                return Err(DTERR(DtasmError::UnknownVariableId(*id))); 
            }
            if self.var_types[id].causality != MD::CausalityType::Input { 
                return Err(DTERR(DtasmError::VariableCausalityInvalidForSet(self.var_types[id].causality, *id))); 
            }
            if self.var_types[id].value_type != MD::VariableType::DtasmInt { 
                return Err(DTERR(DtasmError::VariableTypeMismatch(self.var_types[id].value_type, *id))); 
            }
            var_values.int_values.insert(*id, *val);
        }
        for (id, val) in &input_vals.bool_values {
            if !self.var_types.contains_key(id) { 
                return Err(DTERR(DtasmError::UnknownVariableId(*id))); 
            }
            if self.var_types[id].causality != MD::CausalityType::Input { 
                return Err(DTERR(DtasmError::VariableCausalityInvalidForSet(self.var_types[id].causality, *id))); 
            }
            if self.var_types[id].value_type != MD::VariableType::DtasmBool { 
                return Err(DTERR(DtasmError::VariableTypeMismatch(self.var_types[id].value_type, *id))); 
            }
            var_values.bool_values.insert(*id, *val);
        }
        for (id, val) in &input_vals.string_values {
            if !self.var_types.contains_key(id) { 
                return Err(DTERR(DtasmError::UnknownVariableId(*id))); 
            }
            if self.var_types[id].causality != MD::CausalityType::Input { 
                return Err(DTERR(DtasmError::VariableCausalityInvalidForSet(self.var_types[id].causality, *id))); 
            }
            if self.var_types[id].value_type != MD::VariableType::DtasmString { 
                return Err(DTERR(DtasmError::VariableTypeMismatch(self.var_types[id].value_type, *id))); 
            }
            var_values.string_values.insert(*id, val.clone());
        }

        // build the setValues request message
        let mut real_offs: Vec<flatbuffers::WIPOffset<DTT::RealVal>> = Vec::new();
        for (id, val) in &var_values.real_values {
            real_offs.push(DTT::RealVal::create(&mut self.builder, &DTT::RealValArgs{
                id: *id,
                val: *val
            }));
        }
        let real_vals = self.builder.create_vector(&real_offs);

        let mut int_offs: Vec<flatbuffers::WIPOffset<DTT::IntVal>> = Vec::new();
        for (id, val) in &var_values.int_values {
            int_offs.push(DTT::IntVal::create(&mut self.builder, &DTT::IntValArgs{
                id: *id,
                val: *val
            }));
        }
        let int_vals = self.builder.create_vector(&int_offs);

        let mut bool_offs: Vec<flatbuffers::WIPOffset<DTT::BoolVal>> = Vec::new();
        for (id, val) in &var_values.bool_values {
            bool_offs.push(DTT::BoolVal::create(&mut self.builder, &DTT::BoolValArgs{
                id: *id,
                val: *val
            }));
        }
        let bool_vals = self.builder.create_vector(&bool_offs);

        let mut string_offs: Vec<flatbuffers::WIPOffset<DTT::StringVal>> = Vec::new();
        for (id, val) in &var_values.string_values {
            let val_str = self.builder.create_string(val);
            string_offs.push(DTT::StringVal::create(&mut self.builder, &DTT::StringValArgs{
                id: *id,
                val: Some(val_str)
            }));
        }
        let string_vals = self.builder.create_vector(&string_offs);

        let scalar_vals = DTT::VarValues::create(&mut self.builder, &DTT::VarValuesArgs{
            real_vals: Some(real_vals), 
            int_vals: Some(int_vals),
            bool_vals: Some(bool_vals),
            string_vals: Some(string_vals)
        });

        let set_vals_req = DTAPI::SetValuesReq::create(&mut self.builder, &DTAPI::SetValuesReqArgs{
            values: Some(scalar_vals),
        });
        self.builder.finish(set_vals_req, None);

        let set_req_buf = self.builder.finished_data(); 
        let set_req_len = set_req_buf.len();
        let set_req_ptr = (*self.alloc_fn)(set_req_len as i32)? as usize;

        // copy buffer into allocated position in linear memory
        unsafe {
            self.memory.data_unchecked_mut()[set_req_ptr..set_req_ptr+set_req_len]
                .copy_from_slice(set_req_buf);
        };

        // return value is status only, should fit into 64 bytes
        let size = 64;
        let set_res_ptr = (*self.alloc_fn)(size)? as usize;
        let size_out = (self.set_values_fn)(set_req_ptr as i32, set_req_len as i32, set_res_ptr as i32, size)?;

        if size_out > size { return Err(DTERR(DtasmError::DtasmInternalError(format!("Unexpected size returned from setValues request: {}", size_out)))); }

        let res_bytes = unsafe {
            &self.memory.data_unchecked()[set_res_ptr..set_res_ptr+(size_out as usize)] 
        };

        let init_res = unsafe { FB::root_unchecked::<DTAPI::StatusRes>(res_bytes) };

        let status_res = init_res.status().into();
        
        (*self.dealloc_fn)(set_req_ptr as i32, set_req_len as i32)?;
        (*self.dealloc_fn)(set_res_ptr as i32, size)?;
        self.builder.reset();

        Ok(status_res)
    }

    /// Simulate a time step
    ///
    /// * `current_time` - current time
    /// * `timestep` - step to calculate forward in time
    pub fn do_step(&mut self, current_time: f64, timestep: f64) -> Result<DoStepResponse, DtasmtimeError> {
        // TODO: Check correct state

        // build doStep request message
        let req = DTAPI::DoStepReq::create(&mut self.builder, &DTAPI::DoStepReqArgs{
            current_time: current_time, 
            timestep
        });
        self.builder.finish(req, None);

        let dostep_req_buf = self.builder.finished_data();
        let dostep_req_len = dostep_req_buf.len();
        let dostep_req_ptr = (*self.alloc_fn)(dostep_req_len as i32)? as usize;

        unsafe {
            self.memory.data_unchecked_mut()[dostep_req_ptr..dostep_req_ptr+dostep_req_len]
                .copy_from_slice(dostep_req_buf);
            };
    
        let size = BASE_MEM_SIZE;
        let dostep_res_ptr = (*self.alloc_fn)(size)? as usize;
        let size_out = (*self.do_step_fn)(dostep_req_ptr as i32, dostep_req_len as i32, dostep_res_ptr as i32, size)?;
        
        if size_out > size { return Err(DTERR(DtasmError::DtasmInternalError(format!("Unexpected size returned from doStep request: {}", size_out)))); }
    
        let res_bytes = unsafe {
            &self.memory.data_unchecked()[dostep_res_ptr..dostep_res_ptr+size_out as usize]
        };
    
        let dostep_res = unsafe { FB::root_unchecked::<DTAPI::DoStepRes>(res_bytes) };
        let updated_time = dostep_res.updated_time();
        let status_res = dostep_res.status().into();
     
        (*self.dealloc_fn)(dostep_req_ptr as i32, dostep_req_len as i32)?;
        (*self.dealloc_fn)(dostep_res_ptr as i32, size)?;
        self.builder.reset();

        Ok(DoStepResponse {status: status_res, updated_time})
    }

    fn extract_vals(&getvalues_res: &DTAPI::GetValuesRes, 
        map_id_var: &HashMap<i32, DtasmVarType>) -> Result<DtasmVarValues, DtasmError> {

        let mut var_vals = DtasmVarValues::new();
        
        let values = getvalues_res.values()
            .ok_or(DtasmError::DtasmInternalError("Invalid response received to getValues request: `values` field empty".to_string()))?;
        
        for real_val in values.real_vals().iter().flat_map(identity) {
            let id = real_val.id();
            let val = real_val.val();
    
            if !map_id_var.contains_key(&id){
                return Err(DtasmError::UnknownVariableId(id));
            }
            if map_id_var[&id].value_type != MD::VariableType::DtasmReal {
                return Err(DtasmError::VariableTypeMismatch(MD::VariableType::DtasmReal, id));
            }
            var_vals.real_values.insert(id, val);
        }

        for int_val in values.int_vals().iter().flat_map(identity) {
            let id = int_val.id();
            let val = int_val.val();
    
            if !map_id_var.contains_key(&id){
                return Err(DtasmError::UnknownVariableId(id));
            }
            if map_id_var[&id].value_type != MD::VariableType::DtasmInt {
                return Err(DtasmError::VariableTypeMismatch(MD::VariableType::DtasmInt, id));
            }
            var_vals.int_values.insert(id, val);
        }

        for bool_val in values.bool_vals().iter().flat_map(identity) {
            let id = bool_val.id();
            let val = bool_val.val();
    
            if !map_id_var.contains_key(&id){
                return Err(DtasmError::UnknownVariableId(id));
            }
            if map_id_var[&id].value_type != MD::VariableType::DtasmBool {
                return Err(DtasmError::VariableTypeMismatch(MD::VariableType::DtasmBool, id));
            }
            var_vals.bool_values.insert(id, val);
        }

        for str_val in values.string_vals().iter().flat_map(identity) {
            let id = str_val.id();
            let val = str_val.val().ok_or(DtasmError::InvalidVariableValue("None".to_string(), id))?;
    
            if !map_id_var.contains_key(&id){
                return Err(DtasmError::UnknownVariableId(id));
            }
            if map_id_var[&id].value_type != MD::VariableType::DtasmString {
                return Err(DtasmError::VariableTypeMismatch(MD::VariableType::DtasmString, id));
            }
            var_vals.string_values.insert(id, val.to_string());
        }

        Ok(var_vals)
    }


    fn collect_var_types(md: &MD::ModelDescription) -> Result<HashMap<i32, DtasmVarType>, DtasmtimeError> {
        let model_vars = &md.variables;
        let mut var_types: HashMap<i32, DtasmVarType> = HashMap::new();

        for model_var in model_vars.iter() {
            var_types.insert(model_var.id,
                DtasmVarType {
                    name: model_var.name.clone(), 
                    causality: model_var.causality.clone(),
                    value_type: model_var.value_type.clone(),
                    default: model_var.default.clone()
                });
        }

        Ok(var_types)
    }

    /// Load a serialized state from file into this instance
    pub fn load_state(&self, filepath: PathBuf) -> Result<(), DtasmtimeError>{
        let mut file = std::fs::File::open(filepath)?;

        let mut buffer = Vec::new();
        // read the whole file
        file.read_to_end(&mut buffer)?;

        let state_size = buffer.len() as u32;
        let mem_size = &self.memory.size();

        if state_size > &self.memory.size() * WASM_PAGE_SIZE {
            let add_pages = state_size  / WASM_PAGE_SIZE - mem_size;
            let old_size = &self.memory.grow(add_pages)?;
            assert!(old_size == mem_size, "Memory sizing inconsistency detected");
        }

        unsafe {
            &self.memory.data_unchecked_mut().copy_from_slice(&buffer[..]);
        };

        Ok(())
    }

    /// Serialize the current state of the instance to a binary file
    pub fn save_state(&self, filepath: PathBuf) -> Result<(),DtasmtimeError>{
        let mut file = std::fs::File::create(filepath)?;

        unsafe {
            file.write_all(&self.memory.data_unchecked())?;
        };

        Ok(())
    }
}
