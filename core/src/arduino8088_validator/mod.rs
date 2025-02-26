/*
    MartyPC
    https://github.com/dbalsom/martypc

    Copyright 2022-2023 Daniel Balsom

    Permission is hereby granted, free of charge, to any person obtaining a
    copy of this software and associated documentation files (the “Software”),
    to deal in the Software without restriction, including without limitation
    the rights to use, copy, modify, merge, publish, distribute, sublicense,
    and/or sell copies of the Software, and to permit persons to whom the
    Software is furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in
    all copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER   
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
    FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
    DEALINGS IN THE SOFTWARE.
*/
#![allow(dead_code)]

use std::cmp;

use crate::tracelogger::TraceLogger;
use crate::cpu_808x::{
    QueueOp,
    CPU_FLAG_CARRY,
    CPU_FLAG_PARITY,
    CPU_FLAG_AUX_CARRY,
    CPU_FLAG_ZERO,
    CPU_FLAG_SIGN,
    CPU_FLAG_TRAP,
    CPU_FLAG_INT_ENABLE,
    CPU_FLAG_DIRECTION,
    CPU_FLAG_OVERFLOW
};

mod remote_cpu;
mod queue;
mod udmask;

use crate::arduino8088_client::*;
use crate::cpu_validator::*;
use remote_cpu::*;

const VISIT_ONCE: bool = false;
const NUM_INVALID_FETCHES: usize = 6;
const NUM_MEM_OPS: usize = 0x20000 + 16;
const V_INVALID_POINTER: u32 = 0xFFFFFFFF;
const UPPER_MEMORY: u32 = 0xA0000;
const CYCLE_LIMIT: u32 = 1000;

pub const MOF_UNUSED: u8 = 0x00;
pub const MOF_EMULATOR: u8 = 0x01;
pub const MOF_PI8088: u8 = 0x02;

const DATA_PROGRAM: u8 = 0;
const DATA_FINALIZE: u8 = 1;

const OPCODE_NOP: u8 = 0x90;

macro_rules! trace {
    ($self:ident, $($t:tt)*) => {{
        $self.trace_logger.print(&format!($($t)*));
        $self.trace_logger.print("\n".to_string());
    }};
}

macro_rules! trace_debug {
    ($self:ident, $($t:tt)*) => {{
        log::debug!("{}", &format!($($t)*));
        $self.trace_logger.print(&format!($($t)*));
        $self.trace_logger.print("\n".to_string());
    }};
}

macro_rules! trace_error {
    ($self:ident, $($t:tt)*) => {{
        log::error!("{}", &format!($($t)*));
        $self.trace_logger.print(&format!($($t)*));
        $self.trace_logger.print("\n".to_string());
    }};
}

#[derive (PartialEq, Debug)]
pub enum ValidatorState {
    Setup,
    Execute,
    Readback,
    Finished
}

#[derive (Copy, Clone, Debug, PartialEq)]
pub enum BusOpType {
    CodeRead,
    MemRead,
    MemWrite,
    IoRead,
    IoWrite,
}

#[derive (Copy, Clone)]
pub struct BusOp {
    op_type: BusOpType,
    addr: u32,
    data: u8,
    flags: u8
}

#[derive (Default)]
pub struct InstructionContext {
    name: String,
    instr: Vec<u8>,
    instr_end: usize,
    opcode: u8,
    modrm: u8,
    has_modrm: bool,
    discard: bool,
    next_fetch: bool,

    regs: Vec<VRegisters>,

    emu_prefetch: Vec<BusOp>,
    emu_ops: Vec<BusOp>,
    cpu_prefetch: Vec<BusOp>,
    cpu_ops: Vec<BusOp>,
    mem_op_n: usize,

    cpu_states: Vec<CycleState>
}

impl InstructionContext {
    pub fn new() -> Self {

        Self {
            name: String::new(),
            instr: Vec::new(),
            instr_end: 0,
            opcode: 0,
            modrm: 0,
            has_modrm: false,
            discard: false,
            next_fetch: false,

            regs: vec![VRegisters::default(); 2],

            emu_prefetch: Vec::new(),
            emu_ops: Vec::new(),
            cpu_prefetch: Vec::new(),
            cpu_ops: Vec::new(),
            mem_op_n: 0,
            cpu_states: Vec::new()
        }
    }
}

pub struct ArduinoValidator {

    //cpu_client: Option<CpuClient>,
    mode: ValidatorMode,
    cpu: RemoteCpu,

    current_instr: InstructionContext,
    state: ValidatorState,

    cycle_count: u64,
    do_cycle_trace: bool,

    rd_signal: bool,
    wr_signal: bool,
    iom_signal: bool,
    ale_signal: bool,

    address_latch: u32,

    //cpu_memory_access: AccessType,
    cpu_interrupt_enabled: bool,

    scratchpad: Vec<u8>,
    code_as_data_skip: bool,
    readback_ptr: usize,
    trigger_addr: u32,
    end_addr: usize,

    mask_flags: bool,

    visit_once: bool,
    visited: Vec<bool>,

    log_prefix: String,
    trace_logger: TraceLogger
}

impl ArduinoValidator {

    pub fn new(trace_logger: TraceLogger) -> Self {

        // Trigger addr is address at which to start validation
        // if trigger_addr == V_INVALID_POINTER then validate        
        let trigger_addr = V_INVALID_POINTER;

        let cpu_client = match CpuClient::init() {
            Ok(client) => client,
            Err(e) => {
                panic!("Failed to initialize ArduinoValidator: {}", e);
            }
        };

        ArduinoValidator {
            mode: ValidatorMode::Cycle,
            cpu: RemoteCpu::new(cpu_client),

            current_instr: InstructionContext::new(),
            state: ValidatorState::Setup,

            cycle_count: 0,
            do_cycle_trace: false,
            rd_signal: false,
            wr_signal: false, 
            iom_signal: false,
            ale_signal: false,   
            address_latch: 0,
            //cpu_memory_access: AccessType::AccAlternateData,
            cpu_interrupt_enabled: false,

            scratchpad: vec![0; 0x100000],
            code_as_data_skip: false,
            readback_ptr: 0,
            trigger_addr,
            end_addr: 0,
            mask_flags: true,
            visit_once: VISIT_ONCE,
            visited: vec![false; 0x100000],

            log_prefix: String::new(),
            trace_logger
        }
    }

    pub fn set_end_addr(&mut self, end_addr: usize) {
        self.end_addr = end_addr;
    }

    pub fn regs_to_buf(buf: &mut [u8], regs: &VRegisters) {
        // AX, BX, CX, DX, SS, SP, FLAGS, IP, CS, DS, ES, BP, SI, DI
        buf[0] = (regs.ax & 0xFF) as u8;
        buf[1] = ((regs.ax >> 8) & 0xFF) as u8;

        buf[2] = (regs.bx & 0xFF) as u8;
        buf[3] = ((regs.bx >> 8) & 0xFF) as u8;

        buf[4] = (regs.cx & 0xFF) as u8;
        buf[5] = ((regs.cx >> 8) & 0xFF) as u8;
        
        buf[6] = (regs.dx & 0xFF) as u8;
        buf[7] = ((regs.dx >> 8) & 0xFF) as u8;        

        buf[8] = (regs.ss & 0xFF) as u8;
        buf[9] = ((regs.ss >> 8) & 0xFF) as u8;
        
        buf[10] = (regs.sp & 0xFF) as u8;
        buf[11] = ((regs.sp >> 8) & 0xFF) as u8;
        
        buf[12] = (regs.flags & 0xFF) as u8;
        buf[13] = ((regs.flags >> 8) & 0xFF) as u8;       
        
        buf[14] = (regs.ip & 0xFF) as u8;
        buf[15] = ((regs.ip >> 8) & 0xFF) as u8;
        
        buf[16] = (regs.cs & 0xFF) as u8;
        buf[17] = ((regs.cs >> 8) & 0xFF) as u8;
        
        buf[18] = (regs.ds & 0xFF) as u8;
        buf[19] = ((regs.ds >> 8) & 0xFF) as u8;
        
        buf[20] = (regs.es & 0xFF) as u8;
        buf[21] = ((regs.es >> 8) & 0xFF) as u8;
        
        buf[22] = (regs.bp & 0xFF) as u8;
        buf[23] = ((regs.bp >> 8) & 0xFF) as u8;
        
        buf[24] = (regs.si & 0xFF) as u8;
        buf[25] = ((regs.si >> 8) & 0xFF) as u8;
        
        buf[26] = (regs.di & 0xFF) as u8;
        buf[27] = ((regs.di >> 8) & 0xFF) as u8;
    }

    pub fn buf_to_regs(buf: &[u8]) -> VRegisters {
        VRegisters {
            ax: buf[0] as u16 | ((buf[1] as u16) << 8),
            bx: buf[2] as u16 | ((buf[3] as u16) << 8),
            cx: buf[4] as u16 | ((buf[5] as u16) << 8),
            dx: buf[6] as u16 | ((buf[7] as u16) << 8),
            ss: buf[8] as u16 | ((buf[9] as u16) << 8),
            sp: buf[10] as u16 | ((buf[11] as u16) << 8),
            flags: buf[12]  as u16| ((buf[13] as u16) << 8),
            ip: buf[14] as u16 | ((buf[15] as u16) << 8),
            cs: buf[16] as u16 | ((buf[17] as u16) << 8),
            ds: buf[18] as u16 | ((buf[19] as u16) << 8),
            es: buf[20] as u16 | ((buf[21] as u16) << 8),
            bp: buf[22] as u16 | ((buf[23] as u16) << 8),
            si: buf[24] as u16 | ((buf[25] as u16) << 8),
            di: buf[26] as u16| ((buf[27] as u16) << 8),
        }
    }

    pub fn validate_mem_ops(&mut self, discard: bool) -> bool {

        if discard {
            if self.current_instr.emu_ops.len() > 0 {
                if self.current_instr.emu_ops[0].op_type != BusOpType::CodeRead {
                    trace_error!(self, "Cannot discard op type of {:?}!", self.current_instr.emu_ops[0].op_type);
                    return false;
                }
                else {
                    self.current_instr.emu_ops.remove(0);
                }
            }
            else {
                trace_error!(self, "Discard flag set but no emu ops!");
                return false;
            }
        }

        if self.current_instr.emu_ops.len() != self.current_instr.cpu_ops.len() {
            trace_error!(
                self, 
                "Validator error: Memory op count mismatch. Emu: {} CPU: {}", 
                self.current_instr.emu_ops.len(),
                self.current_instr.cpu_ops.len()
            );

            return false;
        }


        for i in 0..self.current_instr.emu_ops.len() {

            if self.current_instr.emu_ops[i].op_type != self.current_instr.cpu_ops[i].op_type {
                trace_error!(
                    self,
                    "Bus op #{} type mismatch: EMU:{:?} CPU:{:?}",
                    i,
                    self.current_instr.emu_ops[i].op_type,
                    self.current_instr.cpu_ops[i].op_type
                );
                return false;
            }

            if self.current_instr.emu_ops[i].addr != self.current_instr.cpu_ops[i].addr {
                trace_error!(
                    self,
                    "Bus op #{} addr mismatch: EMU:{:?}:{:05X} CPU:{:?}:{:05X}",
                    i,
                    self.current_instr.emu_ops[i].op_type,
                    self.current_instr.emu_ops[i].addr,
                    self.current_instr.cpu_ops[i].op_type,
                    self.current_instr.cpu_ops[i].addr
                );
                return false;
            }

            if self.current_instr.emu_ops[i].data != self.current_instr.cpu_ops[i].data {
                trace_error!(
                    self,
                    "Bus op #{} data mismatch: EMU:{:?}:{:05X} CPU:{:?}:{:05X}",
                    i,
                    self.current_instr.emu_ops[i].op_type,
                    self.current_instr.emu_ops[i].data,
                    self.current_instr.cpu_ops[i].op_type,
                    self.current_instr.cpu_ops[i].data
                );
                return false;
            }                   
        }

        return true;
    }

    pub fn validate_registers(&mut self, regs: &VRegisters) -> bool {

        let mut regs_validate = true;

        if self.current_instr.regs[1].ax != regs.ax {
            regs_validate = false;
        }
        if self.current_instr.regs[1].bx != regs.bx {
            regs_validate = false;
        }
        if self.current_instr.regs[1].cx != regs.cx {
            regs_validate = false;
        }
        if self.current_instr.regs[1].dx != regs.dx {
            regs_validate = false;
        }
        if self.current_instr.regs[1].cs != regs.cs {
            regs_validate = false;
        }
        if self.current_instr.regs[1].ds != regs.ds {
            regs_validate = false;
        }
        if self.current_instr.regs[1].es != regs.es {
            regs_validate = false;
        }
        if self.current_instr.regs[1].sp != regs.sp {
            regs_validate = false;
        }
        if self.current_instr.regs[1].sp != regs.sp {
            regs_validate = false;
        }    
        if self.current_instr.regs[1].bp != regs.bp {
            regs_validate = false;
        }    
        if self.current_instr.regs[1].si != regs.si {
            regs_validate = false;
        }    
        if self.current_instr.regs[1].di != regs.di {
            regs_validate = false;
        }
        
        /*
        if self.current_frame.regs[1] != *regs {
            regs_validate = false;
        }
        */

        let mut emu_flags_masked = self.current_instr.regs[1].flags;
        let mut cpu_flags_masked = regs.flags;

        if self.mask_flags {
            emu_flags_masked = ArduinoValidator::mask_undefined_flags(self.current_instr.opcode, self.current_instr.modrm, self.current_instr.regs[1].flags);
            cpu_flags_masked = ArduinoValidator::mask_undefined_flags(self.current_instr.opcode, self.current_instr.modrm, regs.flags);
        }

        if emu_flags_masked != cpu_flags_masked {

            trace_error!(self, "CPU flags mismatch! EMU: 0b{:08b} != CPU: 0b{:08b}", emu_flags_masked, cpu_flags_masked);
            //trace_error!(self, "Unmasked: EMU: 0b{:08b} != CPU: 0b{:08b}", self.current_frame.regs[1].flags, regs.flags);            
            regs_validate = false;

            let flag_diff = emu_flags_masked ^ cpu_flags_masked;

            if flag_diff & CPU_FLAG_CARRY != 0 {
                trace_error!(self, "CARRY flag differs.");
            }
            if flag_diff & CPU_FLAG_PARITY != 0 {
                trace_error!(self, "PARITY flag differs.");
            }
            if flag_diff & CPU_FLAG_AUX_CARRY != 0 {
                trace_error!(self, "AUX CARRY flag differs.");
            }
            if flag_diff & CPU_FLAG_ZERO != 0 {
                trace_error!(self, "ZERO flag differs.");
            }
            if flag_diff & CPU_FLAG_SIGN != 0 {
                trace_error!(self, "SIGN flag differs.");
            }
            if flag_diff & CPU_FLAG_TRAP != 0 {
                trace_error!(self, "TRAP flag differs.");
            }
            if flag_diff & CPU_FLAG_INT_ENABLE != 0 {
                trace_error!(self, "INT flag differs.");
            }
            if flag_diff & CPU_FLAG_DIRECTION != 0 {
                trace_error!(self, "DIRECTION flag differs.");
            }
            if flag_diff & CPU_FLAG_OVERFLOW != 0 {
                trace_error!(self, "OVERFLOW flag differs.");
            }                    
            //panic!("CPU flag mismatch!")
        }

        regs_validate
    }

    pub fn validate_cycles(
        &mut self, 
        cpu_states: &[CycleState], 
        emu_states: &[CycleState]
    ) -> (bool, usize) {

        if emu_states.len() != cpu_states.len() {
            // Cycle count mismatch
            return (false, 0)
        }

        for i in 0..cpu_states.len() {

            if emu_states[i] != cpu_states[i] {
                // Cycle state mismatch
                return (false, i)
            }
        }

        (true, 0)
    }

    pub fn correct_queue_counts(&mut self, cpu_states: &mut Vec::<CycleState>) {

        for i in 0..cpu_states.len() {

            match cpu_states[i].q_op {
                QueueOp::First | QueueOp::Subsequent => {
                    if i > 0 {
                        // Queue was read out on previous cycle, adjust.
                        cpu_states[i-1].q_len -= 1;
                    }
                }
                QueueOp::Flush => {
                    if i > 0 {
                        // Queue was flushed on previous cycle, adjust.
                        cpu_states[i-1].q_len = 0;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn print_cycle_diff(&mut self, cpu_states: &Vec::<CycleState>, emu_states: &[CycleState]) {

        let max_lines = cmp::max(emu_states.len(), cpu_states.len());

        for i in 0..max_lines {

            let cpu_str;
            let emu_str;
            
            if i < cpu_states.len() {
                cpu_str = RemoteCpu::get_cycle_state_str(&cpu_states[i])
            }
            else {
                cpu_str = String::new();
            }

            if i < emu_states.len() {
                emu_str = RemoteCpu::get_cycle_state_str(&emu_states[i])
            }
            else {
                emu_str = String::new();
            }

            trace!(self, "{:<80} | {:<80}", cpu_str, emu_str);
        }
    }    
}

pub fn make_pointer(base: u16, offset: u16) -> u32 {
    return (((base as u32) << 4) + offset as u32 ) & 0xFFFFF;
}

impl CpuValidator for ArduinoValidator {

    fn init(&mut self, mode: ValidatorMode, mask_flags: bool, cycle_trace: bool, visit_once: bool) -> bool {
        self.mode = mode;
        self.do_cycle_trace = cycle_trace;
        self.mask_flags = mask_flags;
        self.visit_once = visit_once;
        true
    }

    fn reset_instruction(&mut self) {
        self.current_instr.emu_ops.clear();
        self.current_instr.emu_prefetch.clear();
        self.current_instr.cpu_ops.clear();
        self.current_instr.cpu_prefetch.clear();
    }

    fn begin_instruction(&mut self, regs: &VRegisters, end_instr: usize, end_program: usize ) {

        self.current_instr.discard = false;
        self.current_instr.regs[0] = regs.clone();

        //log::debug!(">>> printing regs!");
        //RemoteCpu::print_regs(&self.current_instr.regs[0]);

        let ip_addr = make_pointer(regs.cs, regs.ip);

        //println!("{} : {}", self.trigger_addr, ip_addr);
        if self.trigger_addr == ip_addr {
            log::info!("Trigger address hit, begin validation...");
            self.trigger_addr = V_INVALID_POINTER;
        }

        /*
        if (self.trigger_addr != V_INVALID_POINTER) 
            || (self.visit_once && ip_addr >= UPPER_MEMORY && self.visited[ip_addr as usize]) {
            self.current_frame.discard = true;
            return;
        }
        */

        if let ValidatorMode::Instruction = self.mode {
            // Cannot discard instructions in Cycle mode.
            if self.visit_once && ip_addr >= UPPER_MEMORY && self.visited[ip_addr as usize] {
                log::warn!("Discarding BIOS instruction");
                self.current_instr.discard = true;
            }
        }
 
        self.end_addr = end_program;

        self.current_instr.instr_end = end_instr;
        self.cpu.set_instr_end_addr(end_instr);
        self.cpu.set_program_end_addr(end_program);

    }    

    /// Initialize the physical CPU with a provided register state.
    /// Can only be done after a reset or jump
    fn set_regs(&mut self) {
        trace_debug!(self, "Setting register state...");
        self.cpu.reset();

        let mut reg_buf: [u8; 28] = [0; 28];
        ArduinoValidator::regs_to_buf(&mut reg_buf, &self.current_instr.regs[0]);

        self.cpu.load(&reg_buf).expect("validate() error: Load registers failed.");
    }

    fn validate_instruction(
        &mut self, 
        name: String, 
        instr: &[u8], 
        peek_fetch: u16,
        has_modrm: bool, 
        _cycles: i32, 
        regs: &VRegisters,
        emu_states: &[CycleState],
    ) -> Result<ValidatorResult, ValidatorError>  {

        let ip_addr = make_pointer(self.current_instr.regs[0].cs, self.current_instr.regs[0].ip);

        /*
        if (self.trigger_addr != V_INVALID_POINTER) 
            || (self.visit_once && ip_addr >= UPPER_MEMORY &&  self.visited[ip_addr as usize]) {
            return Ok(true);
        }
        */

        if instr.len() == 0 {
            trace_error!(self, "Instruction length was 0");
            return Err(ValidatorError::ParameterError);
        }

        self.visited[ip_addr as usize] = true;

        let mut i = 0;

        // Scan through prefix bytes to find opcode
        loop {
            let instr_byte = instr[i];
            match instr_byte {
                0x26 | 0x2E | 0x36 | 0x3E | 0xF0 | 0xF2 | 0xF3 => {
                    i += 1;
                }
                _ => {
                    break;
                }
            }
        }

        self.current_instr.name = name.clone();
        self.current_instr.opcode = instr[i];
        self.current_instr.instr = instr.to_vec();
        self.current_instr.has_modrm = has_modrm;

        self.current_instr.next_fetch = false;
        self.current_instr.regs[1] = regs.clone();

        if self.current_instr.regs[1].flags == 0 {
            trace_error!(self, "Invalid emulator flags");
            return Err(ValidatorError::ParameterError);
        }

        //self.current_frame.emu_states.clone_from(&emu_states);
        //RemoteCpu::print_regs(&self.current_instr.regs[0]);

        if has_modrm {
            if i > (instr.len() - 2) {
                trace_error!(self, "validate(): modrm specified but instruction length < ");
                trace_error!(self, 
                    "instruction: {} opcode: {} instr: {:02X?}",
                    self.current_instr.name,
                    self.current_instr.opcode,
                    self.current_instr.instr
                );
                return Err(ValidatorError::ParameterError);
            }
            self.current_instr.modrm = instr[i + 1];
        }
        else {
            self.current_instr.modrm = 0;
        }

        let discard_or_validate = match self.current_instr.discard {
            true => "DISCARD",
            false => "VALIDATE"
        };

        self.cpu.set_instr_string(name.clone());

        trace_debug!(
            self,
            "{}: {} {:02X?} @ [{:04X}:{:04X}] Memops: {} End: {:05X}", 
            discard_or_validate, 
            name, 
            self.current_instr.instr, 
            self.current_instr.regs[0].cs, 
            self.current_instr.regs[0].ip,
            self.current_instr.emu_ops.len(),
            self.current_instr.instr_end
        );

        if self.current_instr.discard {
            return Ok(ValidatorResult::Ok);
        }

        let instr_addr = 
            RemoteCpu::calc_linear_address(
                self.current_instr.regs[0].cs, 
                self.current_instr.regs[0].ip
            );

        let (mut cpu_states, discard) = self.cpu.step(
            &self.current_instr.instr,
            instr_addr,
            self.do_cycle_trace,
            peek_fetch,
            &mut self.current_instr.emu_prefetch, 
            &mut self.current_instr.emu_ops, 
            &mut self.current_instr.cpu_prefetch, 
            &mut self.current_instr.cpu_ops,
            &mut self.trace_logger
        )?;

        if self.current_instr.opcode != 0x9C {
            // We ignore PUSHF results due to undefined flags causing write mismatches
            if !self.validate_mem_ops(discard) {
    
                trace_error!(self, "Memory validation failure. EMU:");
                RemoteCpu::print_regs(&self.current_instr.regs[1]);
                trace_error!(self, "CPU:");    
                RemoteCpu::print_regs(&regs);
    
                self.print_cycle_diff(&cpu_states, &emu_states);
                self.trace_logger.flush();
    
                return Err(ValidatorError::MemOpMismatch);            
            }
        }


        if emu_states.len() > 0 {
            // Only validate CPU cycles if any were provided

            self.correct_queue_counts(&mut cpu_states);
            let (result, cycle_num) = self.validate_cycles(&cpu_states, &emu_states);

            if !result {
                trace_error!(self, "Cycle state validation failure @ cycle {}", cycle_num);    
                self.print_cycle_diff(&cpu_states, &emu_states);
                trace_error!(self, "EMU AFTER:");
                RemoteCpu::print_regs(&self.current_instr.regs[1]);
    
                trace_error!(self, "CPU AFTER:");   
                RemoteCpu::print_regs(&regs);       
                self.trace_logger.flush();

                return Err(ValidatorError::CycleMismatch);
            }
            else {
                self.print_cycle_diff(&cpu_states, &emu_states);
            }
        }

        self.reset_instruction();

        // Did this instruction enter finalize state?
        if self.cpu.in_finalize() {
            trace!(self, " >>> Validator finalizing!");
            Ok(ValidatorResult::OkEnd)   
        }
        else {
            trace!(self, " >>> Validator finished validating instruction");
            Ok(ValidatorResult::Ok)
        }
    }

    fn validate_regs(&mut self, regs: &VRegisters) -> Result<(), ValidatorError> {

        let mut store_regs = self.cpu.store().expect("Failed to store registers!");
        self.cpu.adjust_ip(&mut store_regs);

        if !self.validate_registers(&regs) {
            trace_error!(self, "Register validation failure. EMU BEFORE:");    
            RemoteCpu::print_regs(&self.current_instr.regs[0]);
            trace_error!(self, "EMU AFTER:");
            RemoteCpu::print_regs(&self.current_instr.regs[1]);

            trace_error!(self, "CPU AFTER:");   
            RemoteCpu::print_regs(&regs);

            return Err(ValidatorError::RegisterMismatch);
        }

        Ok(())
    }

    fn emu_read_byte(&mut self, addr: u32, data: u8, bus_type: BusType, read_type: ReadType) {
        if self.current_instr.discard {
            return;
        }
        
        match (bus_type, read_type) { 
            (BusType::Mem, ReadType::Code) => {
                self.current_instr.emu_ops.push(
                    BusOp {
                        op_type: BusOpType::CodeRead,
                        addr,
                        data,
                        flags: MOF_EMULATOR
                    }
                );
                trace!(self, "EMU fetch: [{:05X}] -> 0x{:02X} ({})", addr, data, self.current_instr.emu_ops.len());
            }
            (BusType::Mem, ReadType::Data) => {
                self.current_instr.emu_ops.push(
                    BusOp {
                        op_type: BusOpType::MemRead,
                        addr,
                        data,
                        flags: MOF_EMULATOR
                    }
                );
                trace!(self, "EMU read: [{:05X}] -> 0x{:02X} ({})", addr, data, self.current_instr.emu_ops.len());
            }
            (BusType::Io, _) => {
                self.current_instr.emu_ops.push(
                    BusOp {
                        op_type: BusOpType::IoRead,
                        addr,
                        data,
                        flags: MOF_EMULATOR
                    }
                );
                trace!(self, "EMU IN: [{:05X}] -> 0x{:02X} ({})", addr, data, self.current_instr.emu_ops.len());
            }
        }

    }

    fn emu_write_byte(&mut self, addr: u32, data: u8, bus_type: BusType) {
        
        self.visited[(addr & 0xFFFFF) as usize] = false;
        
        if self.current_instr.discard {
            return;
        }

        match bus_type {
            BusType::Mem => {
                self.current_instr.emu_ops.push(
                    BusOp {
                        op_type: BusOpType::MemWrite,
                        addr,
                        data,
                        flags: MOF_EMULATOR
                    }
                );

                trace!(self, "EMU write: [{:05X}] <- 0x{:02X}", addr, data);
            }
            BusType::Io => {
                self.current_instr.emu_ops.push(
                    BusOp {
                        op_type: BusOpType::IoWrite,
                        addr,
                        data,
                        flags: MOF_EMULATOR
                    }
                );

                trace!(self, "EMU OUT: [{:05X}] <- 0x{:02X}", addr, data);
            }
        }
    }

    fn discard_op(&mut self) {
        self.current_instr.discard = true;
    }

    fn flush(&mut self) {
        self.trace_logger.flush();
    }
}
