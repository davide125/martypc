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

    ---------------------------------------------------------------------------

    cpu_808x::addressing.rs

    Implements addressing mode and operand loading routines.

*/

use crate::cpu_808x::*;
use crate::cpu_808x::biu::*;

#[derive(Copy, Clone, Debug)]
pub enum AddressingMode {
    BxSi,
    BxDi,
    BpSi,
    BpDi,
    Si,
    Di,
    Disp16(Displacement),
    Bx,
    BxSiDisp8(Displacement),
    BxDiDisp8(Displacement),
    BpSiDisp8(Displacement),
    BpDiDisp8(Displacement),
    SiDisp8(Displacement),
    DiDisp8(Displacement),
    BpDisp8(Displacement),
    BxDisp8(Displacement),
    BxSiDisp16(Displacement),
    BxDiDisp16(Displacement),
    BpSiDisp16(Displacement),
    BpDiDisp16(Displacement),
    SiDisp16(Displacement),
    DiDisp16(Displacement),
    BpDisp16(Displacement),
    BxDisp16(Displacement),
    RegisterMode
}

#[derive(Copy, Clone, Debug)]
pub enum FarPtr {
    Offset,
    Segment
}

impl Cpu {

    #[allow(dead_code)]
    fn is_register_mode(mode: AddressingMode) {
        match mode {
            AddressingMode::RegisterMode => true,
            _ => false
        };
    }

    pub fn calc_linear_address(segment: u16, offset: u16) -> u32 {
        (((segment as u32) << 4) + offset as u32) & 0xFFFFFu32
    }

    pub fn relative_offset_u16(base: u16, offset: i16) -> u16 {
        base.wrapping_add(offset as u16)
    }
    
    pub fn calc_linear_address_seg(&self, segment: Segment, offset: u16) -> u32 {

        let segment_val: u16 = match segment {
            Segment::None => 0,
            Segment::ES => self.es,
            Segment::CS => self.cs,
            Segment::DS => self.ds,
            Segment::SS => self.ss,
        };
        (((segment_val as u32) << 4) + offset as u32) & 0xFFFFFu32
    }

    pub fn segment_override(seg_override: SegmentOverride, seg_default: Segment) -> Segment {
        match seg_override {
            SegmentOverride::None => seg_default,
            SegmentOverride::ES => Segment::ES,
            SegmentOverride::CS => Segment::CS,
            SegmentOverride::SS => Segment::SS,
            SegmentOverride::DS => Segment::DS
        }
    }

    /// Calculate the Effective Address for the given AddressingMode enum
    pub fn calc_effective_address(
        &mut self, 
        mode: AddressingMode, 
        segment_override: SegmentOverride) 
            -> (u16, Segment, u16) 
    {
        // Addressing modes that reference BP use the stack segment instead of data segment 
        // unless a segment override is present.

        // Using a segment override adds 2 cycles per Intel documentation.
        // Addressing mode calculations are performed on the ALU and are expensive!
        // -- Mod 0x00 ------ cycles --
        // ds:[bx+si]        7 cycles
        // ds:[bx+di]        8 cycles
        // ss:[bp+si]        8 cycles
        // ss:[bp+di]        7 cycles
        // ds:[si]           5 cycles
        // ds:[di]           5 cycles       
        // ds:[disp]         6 cycles
        // ds:[bx]           5 cycles
        // -- Mod 0x01 ------ cycles --
        // ds:[bx+si+{disp]  11 cycles
        // ds:[bx+di+{disp]  12 cycles
        // ss:[bp+si+{disp]  12 cycles
        // ss:[bp+di+{disp]  11 cycles
        // ds:[si+disp]      9 cycles
        // ds:[di+disp]      9 cycles
        // ss:[bp+disp]      9 cycles
        // ds:[bx+disp]      9 cycles
        // -- Mod 0x10 ------ cycles --
        // ds:[bx+si+disp]   11 cycles
        // ds:[bx+di+disp]   12 cycles
        // ss:[bp+si+disp]   12 cycles
        // ss:[bp+si+disp]   8 cycles
        // ds:[si+disp]      
        // ds:[di+disp]
        // ss:[bp+disp]
        // ds:[bx+disp]
        // Reference: (210912·001, Table 1-15)

        // Override default segments based on prefix
        let (segment_value_base_ds, segment_base_ds) = match segment_override {
            SegmentOverride::None => (self.ds, Segment::DS),
            SegmentOverride::ES  => (self.es, Segment::ES),
            SegmentOverride::CS  => (self.cs, Segment::CS),
            SegmentOverride::SS  => (self.ss, Segment::SS),
            SegmentOverride::DS  => (self.ds, Segment::DS),
        };      

        let (segment_value_base_ss, segment_base_ss) = match segment_override {
            SegmentOverride::None => (self.ss, Segment::SS),
            SegmentOverride::ES  => (self.es, Segment::ES),
            SegmentOverride::CS  => (self.cs, Segment::CS),
            SegmentOverride::SS  => (self.ss, Segment::SS),
            SegmentOverride::DS  => (self.ds, Segment::DS),
        };      

        let (seg_val, seg, offset) = match mode {
            // All of this relies on 2's compliment arithmetic for signed displacements
            AddressingMode::BxSi                => (segment_value_base_ds, segment_base_ds, self.bx.wrapping_add(self.si)),
            AddressingMode::BxDi                => (segment_value_base_ds, segment_base_ds, self.bx.wrapping_add(self.di)),
            AddressingMode::BpSi                => (segment_value_base_ss, segment_base_ss, self.bp.wrapping_add(self.si)),  // BP -> SS default seg
            AddressingMode::BpDi                => (segment_value_base_ss, segment_base_ss, self.bp.wrapping_add(self.di)),  // BP -> SS default seg
            AddressingMode::Si                  => (segment_value_base_ds, segment_base_ds, self.si),
            AddressingMode::Di                  => (segment_value_base_ds, segment_base_ds, self.di),
            AddressingMode::Disp16(disp16)      => (segment_value_base_ds, segment_base_ds, disp16.get_u16()),
            AddressingMode::Bx                  => (segment_value_base_ds, segment_base_ds, self.bx),
            
            AddressingMode::BxSiDisp8(disp8)    => (segment_value_base_ds, segment_base_ds, self.bx.wrapping_add(self.si.wrapping_add(disp8.get_u16()))),
            AddressingMode::BxDiDisp8(disp8)    => (segment_value_base_ds, segment_base_ds, self.bx.wrapping_add(self.di.wrapping_add(disp8.get_u16()))),
            AddressingMode::BpSiDisp8(disp8)    => (segment_value_base_ss, segment_base_ss, self.bp.wrapping_add(self.si.wrapping_add(disp8.get_u16()))),  // BP -> SS default seg
            AddressingMode::BpDiDisp8(disp8)    => (segment_value_base_ss, segment_base_ss, self.bp.wrapping_add(self.di.wrapping_add(disp8.get_u16()))),  // BP -> SS default seg
            AddressingMode::SiDisp8(disp8)      => (segment_value_base_ds, segment_base_ds, self.si.wrapping_add(disp8.get_u16())),
            AddressingMode::DiDisp8(disp8)      => (segment_value_base_ds, segment_base_ds, self.di.wrapping_add(disp8.get_u16())),
            AddressingMode::BpDisp8(disp8)      => (segment_value_base_ss, segment_base_ss, self.bp.wrapping_add(disp8.get_u16())),    // BP -> SS default seg
            AddressingMode::BxDisp8(disp8)      => (segment_value_base_ds, segment_base_ds, self.bx.wrapping_add(disp8.get_u16())),
            
            AddressingMode::BxSiDisp16(disp16)  => (segment_value_base_ds, segment_base_ds, self.bx.wrapping_add(self.si.wrapping_add(disp16.get_u16()))),
            AddressingMode::BxDiDisp16(disp16)  => (segment_value_base_ds, segment_base_ds, self.bx.wrapping_add(self.di.wrapping_add(disp16.get_u16()))),
            AddressingMode::BpSiDisp16(disp16)  => (segment_value_base_ss, segment_base_ss, self.bp.wrapping_add(self.si.wrapping_add(disp16.get_u16()))), // BP -> SS default reg
            AddressingMode::BpDiDisp16(disp16)  => (segment_value_base_ss, segment_base_ss, self.bp.wrapping_add(self.di.wrapping_add(disp16.get_u16()))), // BP -> SS default reg
            AddressingMode::SiDisp16(disp16)    => (segment_value_base_ds, segment_base_ds, self.si.wrapping_add(disp16.get_u16())),
            AddressingMode::DiDisp16(disp16)    => (segment_value_base_ds, segment_base_ds, self.di.wrapping_add(disp16.get_u16())),
            AddressingMode::BpDisp16(disp16)    => (segment_value_base_ss, segment_base_ss, self.bp.wrapping_add(disp16.get_u16())),   // BP -> SS default reg
            AddressingMode::BxDisp16(disp16)    => (segment_value_base_ds, segment_base_ds, self.bx.wrapping_add(disp16.get_u16())),

            // The instruction decoder should convert ModRM operands that specify Registers to Register type operands, so
            // in theory this shouldn't happen
            AddressingMode::RegisterMode => panic!("Can't calculate EA for register")
        };

        self.last_ea = offset; // Save last EA to do voodoo when LEA is called with reg, reg operands
        (seg_val, seg, offset)
    }

    pub fn load_effective_address(&mut self, operand: OperandType) -> Option<u16> {
        if let OperandType::AddressingMode(mode) = operand {
            let (_segment_value, _segment, offset) = self.calc_effective_address(mode, SegmentOverride::None);
            return Some(offset);
        }
        None
    }

    /// Load the EA operand for the current instruction, if applicable
    /// (not all instructions with a mod r/m will load, ie, write-only instructions)
    pub fn load_operand(&mut self) {
        if self.i.flags & I_LOAD_EA != 0 {
            // This instruction loads its EA operand. Load and save into OPR.

            let ea_mode: AddressingMode;
            let ea_size;
            if let OperandType::AddressingMode(mode) = self.i.operand1_type {
                ea_size = self.i.operand1_size;
                ea_mode = mode;
            }
            else if let OperandType::AddressingMode(mode) = self.i.operand2_type {
                ea_size = self.i.operand2_size;
                ea_mode = mode;
            }
            else {
                return;
            }

            self.mc_pc = 0x1e0; // EALOAD - 1
            let (_segment_val, segment, offset) = self.calc_effective_address(ea_mode, self.i.segment_override);
            let flat_addr = self.calc_linear_address_seg(segment, offset);

            self.trace_comment("EALOAD");

            /*
            // We can use the width bit (bit 0) of the opcode to determine size of operand when a modrm is present,
            // but 8C and 8E are exceptions to the rule...
            let wide = match self.i.opcode {
                0xC4 | 0x8C | 0x8E => true,
                _ => self.i.opcode & 0x01 == 1 
            };
            */

            if ea_size == OperandSize::Operand16 {
                // Width is word
                assert!(ea_size == OperandSize::Operand16);
                self.ea_opr = self.biu_read_u16(segment, flat_addr, ReadWriteFlag::Normal);
            }
            else {
                // Width is byte
                assert!(ea_size == OperandSize::Operand8);
                self.ea_opr = self.biu_read_u8(segment, flat_addr) as u16;
            }
            self.cycles_i(2, &[0x1e2, MC_RTN]); // Return delay cycle from EALOAD            
        }
    }

    /// Return the value of an 8-bit Operand
    pub fn read_operand8(&mut self, operand: OperandType, seg_override: SegmentOverride) -> Option<u8> {

        // The operand enums contain values peeked from instruction fetch. However for accurate cycle
        // timing, we have to fetch them again now.

        // Ideally we would assert that the peeked operand values equal the fetched values, but this can 
        // fail with self-modifying code, such as the end credits of 8088mph.

        match operand {
            OperandType::Immediate8(_imm8) => {
                let byte = self.q_read_u8(QueueType::Subsequent, QueueReader::Eu);
                Some(byte)
            }
            OperandType::Immediate8s(_imm8s) => {
                let byte = self.q_read_i8(QueueType::Subsequent, QueueReader::Eu);
                Some(byte as u8)
            }
            OperandType::Relative8(_rel8) => {
                let byte = self.q_read_i8(QueueType::Subsequent, QueueReader::Eu);
                Some(byte as u8)
            }
            OperandType::Offset8(_offset8) => {
                let offset = self.q_read_u16(QueueType::Subsequent, QueueReader::Eu);

                let segment = Cpu::segment_override(seg_override, Segment::DS);
                let flat_addr = self.calc_linear_address_seg(segment, offset);
                let byte = self.biu_read_u8(segment, flat_addr);
                Some(byte)
            },
            OperandType::Register8(reg8) => {
                match reg8 {
                    Register8::AH => Some(self.ah),
                    Register8::AL => Some(self.al),
                    Register8::BH => Some(self.bh),
                    Register8::BL => Some(self.bl),
                    Register8::CH => Some(self.ch),
                    Register8::CL => Some(self.cl),
                    Register8::DH => Some(self.dh),
                    Register8::DL => Some(self.dl)
                }
            },
            OperandType::AddressingMode(_mode) => {
                // EA operand was already fetched into ea_opr. Return masked byte.
                if self.i.opcode & 0x01 != 0 {
                    panic!("Reading byte operand for word size instruction");
                }
                Some((self.ea_opr & 0xFF) as u8)
            }
            _=> None
        }
    }

    /// Return the value of a 16-bit Operand
    pub fn read_operand16(&mut self, operand: OperandType, seg_override: SegmentOverride) -> Option<u16> {

        // The operand enums contain values peeked from instruction fetch. However for accurate cycle
        // timing, we have to fetch them again now.

        // Ideally we would assert that the peeked operand values equal the fetched values, but this can 
        // fail with self-modifying code, such as the end credits of 8088mph.

        match operand {
            OperandType::Immediate16(_imm16) => {
                let word = self.q_read_u16(QueueType::Subsequent, QueueReader::Eu);
                Some(word)                
            },
            OperandType::Relative16(_rel16) => {
                let word = self.q_read_i16(QueueType::Subsequent, QueueReader::Eu);
                Some(word as u16)
            }
            OperandType::Offset16(_offset16) => {
                let offset = self.q_read_u16(QueueType::Subsequent, QueueReader::Eu);

                let segment = Cpu::segment_override(seg_override, Segment::DS);
                let flat_addr = self.calc_linear_address_seg(segment, offset);
                let word = self.biu_read_u16(segment, flat_addr, ReadWriteFlag::Normal);

                Some(word)
            }
            OperandType::Register16(reg16) => {
                match reg16 {
                    Register16::AX => Some(self.ax),
                    Register16::CX => Some(self.cx),
                    Register16::DX => Some(self.dx),
                    Register16::BX => Some(self.bx),
                    Register16::SP => Some(self.sp),
                    Register16::BP => Some(self.bp),
                    Register16::SI => Some(self.si),
                    Register16::DI => Some(self.di),
                    Register16::ES => Some(self.es),
                    Register16::CS => Some(self.cs),
                    Register16::SS => Some(self.ss),
                    Register16::DS => Some(self.ds),
                    _=> panic!("read_operand16(): Invalid Register16 operand: {:?}", reg16)
                }
            },
            OperandType::AddressingMode(_mode) => {
                // EA operand was already fetched into ea_opr. Return it.             
                Some(self.ea_opr)
            }
            _ => None
        }
    }    

    /// Load a far address operand from instruction queue and return the segment, offset tuple.
    pub fn read_operand_faraddr(&mut self) -> (u16, u16) {

        let o1 = self.biu_queue_read(QueueType::Subsequent, QueueReader::Eu);
        let o2 = self.biu_queue_read(QueueType::Subsequent, QueueReader::Eu);
        let s1 = self.biu_queue_read(QueueType::Subsequent, QueueReader::Eu);
        let s2 = self.biu_queue_read(QueueType::Subsequent, QueueReader::Eu);

        ((s1 as u16) | (s2 as u16) << 8, (o1 as u16) | (o2 as u16) << 8)
    }

    pub fn read_operand_farptr(&mut self, operand: OperandType, seg_override: SegmentOverride, flag: ReadWriteFlag) -> Option<(u16, u16)> {

        match operand {
            OperandType::AddressingMode(mode) => {
                let offset = self.ea_opr;

                let (segment_val, segment, ea_offset) = self.calc_effective_address(mode, seg_override);
                let flat_addr = Cpu::calc_linear_address(segment_val, ea_offset);
                let segment = self.biu_read_u16(segment, flat_addr + 2, flag);
                Some((segment, offset))
            },
            OperandType::Register16(_) => {
                // Illegal form of LES/LDS reg reg uses the last calculated EA.
                let (segment_value_base_ds, segment_base_ds) = match self.i.segment_override {
                    SegmentOverride::None => (self.ds, Segment::DS),
                    SegmentOverride::ES  => (self.es, Segment::ES),
                    SegmentOverride::CS  => (self.cs, Segment::CS),
                    SegmentOverride::SS  => (self.ss, Segment::SS),
                    SegmentOverride::DS  => (self.ds, Segment::DS),
                };

                let flat_addr = Cpu::calc_linear_address(segment_value_base_ds, self.last_ea);
                let flat_addr2 = Cpu::calc_linear_address(segment_value_base_ds, self.last_ea.wrapping_add(2));

                let offset = self.biu_read_u16(segment_base_ds, flat_addr, ReadWriteFlag::Normal);
                let segment = self.biu_read_u16(segment_base_ds, flat_addr2, ReadWriteFlag::Normal);
                Some((segment, offset))
            },
            _ => None
        }
    }    

    pub fn read_operand_farptr2(&mut self, operand: OperandType, seg_override: SegmentOverride, ptr: FarPtr, flag: ReadWriteFlag) -> Option<u16> {

        match operand {
            OperandType::AddressingMode(mode) => {
                let (segment_val, segment, offset) = self.calc_effective_address(mode, seg_override);
                let flat_addr = Cpu::calc_linear_address(segment_val, offset);

                match ptr {
                    FarPtr::Offset => Some(self.biu_read_u16(segment, flat_addr, flag)),
                    FarPtr::Segment => Some(self.biu_read_u16(segment, flat_addr.wrapping_add(2), flag))
                }
            },
            OperandType::Register16(_) => {
                // Illegal form of LES/LDS reg reg uses the last calculated EA.
                let (segment_value_base_ds, segment_base_ds) = match self.i.segment_override {
                    SegmentOverride::None => (self.ds, Segment::DS),
                    SegmentOverride::ES  => (self.es, Segment::ES),
                    SegmentOverride::CS  => (self.cs, Segment::CS),
                    SegmentOverride::SS  => (self.ss, Segment::SS),
                    SegmentOverride::DS  => (self.ds, Segment::DS),
                };

                //let _flat_addr = Cpu::calc_linear_address(segment_value_base_ds, self.last_ea);
                let flat_addr2 = Cpu::calc_linear_address(segment_value_base_ds, self.last_ea.wrapping_add(2));

                match ptr {
                    FarPtr::Offset => Some(0),
                    FarPtr::Segment => Some(self.biu_read_u16(segment_base_ds, flat_addr2, flag))
                }
            },
            _ => None
        }
    }    

    /// Write an 8-bit value to the specified destination operand
    pub fn write_operand8(&mut self, operand: OperandType, seg_override: SegmentOverride, value: u8, flag: ReadWriteFlag) {

        match operand {
            OperandType::Offset8(_offset8) => {
                let offset = self.q_read_u16(QueueType::Subsequent, QueueReader::Eu);
                self.cycle();

                let segment = Cpu::segment_override(seg_override, Segment::DS);
                let flat_addr = self.calc_linear_address_seg(segment, offset);
                self.biu_write_u8(segment, flat_addr, value, flag);
            }
            OperandType::Register8(reg8) => {
                match reg8 {
                    Register8::AH => self.set_register8(Register8::AH, value),
                    Register8::AL => self.set_register8(Register8::AL , value),
                    Register8::BH => self.set_register8(Register8::BH, value),
                    Register8::BL => self.set_register8(Register8::BL, value),
                    Register8::CH => self.set_register8(Register8::CH, value),
                    Register8::CL => self.set_register8(Register8::CL , value),
                    Register8::DH => self.set_register8(Register8::DH , value),
                    Register8::DL => self.set_register8(Register8::DL , value),
                }
            },
            OperandType::AddressingMode(mode) => {
                let (_segment_val, segment, offset) = self.calc_effective_address(mode, seg_override);
                let flat_addr = self.calc_linear_address_seg(segment, offset);
                self.biu_write_u8(segment, flat_addr, value, flag);
            }
            _ => {}
        }
    }

    // TODO: implement cycle cost
    pub fn write_operand16(&mut self, operand: OperandType, seg_override: SegmentOverride, value: u16, flag: ReadWriteFlag) {

        match operand {
            OperandType::Offset16(_offset16) => {
                let offset = self.q_read_u16(QueueType::Subsequent, QueueReader::Eu);
                self.cycle();

                let segment = Cpu::segment_override(seg_override, Segment::DS);
                let flat_addr = self.calc_linear_address_seg(segment, offset);
                self.biu_write_u16(segment, flat_addr, value, flag);
            }
            OperandType::Register16(reg16) => {
                match reg16 {
                    Register16::AX => self.set_register16(Register16::AX, value),
                    Register16::CX => self.set_register16(Register16::CX, value),
                    Register16::DX => self.set_register16(Register16::DX, value),
                    Register16::BX => self.set_register16(Register16::BX, value),
                    Register16::SP => self.set_register16(Register16::SP, value),
                    Register16::BP => self.set_register16(Register16::BP, value),
                    Register16::SI => self.set_register16(Register16::SI, value),
                    Register16::DI => self.set_register16(Register16::DI, value),
                    Register16::ES => self.set_register16(Register16::ES, value),
                    Register16::CS => {
                        self.biu_update_cs(value); // Update the PC for the new CS segment.
                    },
                    Register16::SS => {
                        self.set_register16(Register16::SS, value);
                        // Technically only MOV ss, nn instructions will inhibit interrupts for one instruction
                        // Other writes may not. 
                        self.interrupt_inhibit = true;
                    },
                    Register16::DS => self.set_register16(Register16::DS, value),
                    _=> panic!("read_operand16(): Invalid Register16 operand")
                }
            }
            OperandType::AddressingMode(mode) => {
                let (_segment_val, segment, offset) = self.calc_effective_address(mode, seg_override);
                let flat_addr = self.calc_linear_address_seg(segment, offset);
                self.biu_write_u16(segment, flat_addr, value, flag);
            }
            _ => {}
        }
    }    
}