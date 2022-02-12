// Copyright 2021 Siemens AG
// SPDX-License-Identifier: MIT

use dtasm_base::{types,model_description};

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::interface::DtasmIf;
use model_description::{ModelDescription};
use types::{DoStepResponse, DtasmVarValues, GetValuesResponse, Status};
use crate::errors::DtasmError;

use crate::add_types::{AddState, AddVar, create_var_maps};

#[no_mangle]
pub static mut SIM_MODULE: Option<Box<dyn DtasmIf + Sync + Send>> = None;
static mut ADD_STATE: Option<AddState> = None;


pub struct AddMod;

impl DtasmIf for AddMod {
    fn get_model_description(&mut self) -> Option<&'static [u8]> {
        Some(include_bytes!("../target/modelDescription.fb"))
    }

    fn initialize(&mut self, md: &ModelDescription, _initial_vals: &types::DtasmVarValues, tmin: f64, _tmax: Option<f64>, 
        _tol: Option<f64>, _log_level: types::LogLevel, _check: bool) -> Result<Status, DtasmError> {
        
        unsafe { ADD_STATE = Some(AddState::new()); }
        let state: &mut AddState;

        unsafe { state = ADD_STATE.as_mut().unwrap(); }

        state.t = tmin;

        create_var_maps(&md, &mut state.var_maps);
        
        Ok(Status::OK)
    }

    fn get_values(&self, var_ids: &Vec<i32>) -> Result<GetValuesResponse, DtasmError> {
        let state: &AddState;

        unsafe { state = ADD_STATE.as_ref().unwrap(); }

        let mut var_vals = DtasmVarValues::new();

        for id in var_ids {
            if !state.var_maps.map_id_var.contains_key(&id) {
                return Err(DtasmError::UnknownVariableId);
            }

            let var = state.var_maps.map_id_var[&id];
            match var {
                AddVar::IO => { 
                    var_vals.int_values.insert(*id, state.int_values[&var]);
                }
                AddVar::RO => {
                    var_vals.real_values.insert(*id, state.real_values[&var]);
                }
                AddVar::BO => {
                    var_vals.bool_values.insert(*id, state.bool_values[&var]);
                }
                _ => { return Err(DtasmError::UnknownVariableId); }
            }
        }

        Ok(GetValuesResponse{
            current_time: state.t, 
            status: Status::OK, 
            values: var_vals
        })
    }

    fn set_values(&mut self, input_vals: &types::DtasmVarValues) -> Result<types::Status, DtasmError> {
        let state: &mut AddState;

        unsafe { state = ADD_STATE.as_mut().unwrap(); }
        
        for id in input_vals.real_values.keys(){
            let var = state.var_maps.map_id_var[id];
            let val = input_vals.real_values[id];
            state.real_values.insert(var, val);
        }

        for id in input_vals.int_values.keys(){
            let var = state.var_maps.map_id_var[id];
            let val = input_vals.int_values[id];
            state.int_values.insert(var, val);
        }

        for id in input_vals.bool_values.keys(){
            let var = state.var_maps.map_id_var[id];
            let val = input_vals.bool_values[id];
            state.bool_values.insert(var, val);
        }

        Ok(types::Status::OK)
    }

    fn do_step(&mut self, _current_time: f64, timestep: f64) -> Result<types::DoStepResponse, DtasmError> {
        let state: &mut AddState;

        unsafe { state = ADD_STATE.as_mut().unwrap(); }
        
        let r_out = state.real_values[&AddVar::RI1] + state.real_values[&AddVar::RI2];
        state.real_values.insert(AddVar::RO, r_out);

        let i_out = state.int_values[&AddVar::II1] + state.int_values[&AddVar::II2];
        state.int_values.insert(AddVar::IO, i_out);

        let b_out = state.bool_values[&AddVar::BI1] && state.bool_values[&AddVar::BI2];
        state.bool_values.insert(AddVar::BO, b_out);

        state.t += timestep;
        
        let do_step_res = DoStepResponse {
            status: Status::OK, 
            updated_time: state.t
        }; 

        Ok(do_step_res)
    }
} 

