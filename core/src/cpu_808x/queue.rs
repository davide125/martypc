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

    cpu_808x::queue.rs

    Implements the data structure for the processor instruction queue.

*/

use crate::cpu_808x::*;
use crate::bytequeue::*;

#[derive (Copy, Clone, PartialEq)]
pub enum QueueDelay {
    Read,
    Write,
    None
}

impl Default for QueueDelay {
    fn default() -> Self {
        QueueDelay::None
    }
}

pub struct InstructionQueue {
    size: usize,
    len: usize,
    back: usize,
    front: usize,
    q: [u8; QUEUE_MAX],
    _dt: [QueueType; QUEUE_MAX],
    preload: Option<u8>,
    delay: QueueDelay
}

impl Default for InstructionQueue {
    fn default() -> Self {
        Self::new(4)
    }
}

impl InstructionQueue {
    pub fn new(size: usize) -> Self {
        Self {
            size,
            len: 0,
            back: 0,
            front: 0,
            q: [0; QUEUE_MAX],
            _dt: [QueueType::First; QUEUE_MAX],
            preload: None,
            delay: QueueDelay::None,
        }
    }

    pub fn set_size(&mut self, size: usize) {
        assert!(size <= QUEUE_MAX);
        self.size = size;
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[allow(dead_code)]
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len == self.size
    }

    #[inline]
    pub fn get_preload(&mut self) -> Option<u8> {
        let preload = self.preload;
        self.preload = None;
        preload
    }

    #[inline]
    pub fn has_preload(&self) -> bool {
        if let Some(_) = self.preload {
            true
        }
        else {
            false
        }
    }

    #[inline]
    pub fn set_preload(&mut self) {
        if self.len > 0 {
            let byte = self.pop();
            self.preload = Some(byte);
        }
        else {
            panic!("Tried to preload with empty queue.")
        }
    }

    pub fn push8(&mut self, byte: u8) {
        if self.len < self.size {

            self.q[self.front] = byte;
            //self.dt[self.front] = dtype;

            self.front = (self.front + 1) % self.size;
            self.len += 1;

            if self.len == 3 {
                // Queue length of 3 after push. Set delay flag A.
                // TODO: Handle 8086? We should set delay on 4 as well(?)
                self.delay = QueueDelay::Write;
            }
            else {
                self.delay = QueueDelay::None;
            }            
        }
        else {
            panic!("Queue overrun!");
        }
    }

    pub fn push16(&mut self, word: u16) {

        self.push8((word & 0xFF) as u8);
        self.push8(((word >> 8) & 0xFF) as u8);
    }

    pub fn pop(&mut self) -> u8 {
        if self.len > 0 {
            let byte = self.q[self.back];
            //let dt = self.dt[self.back];

            self.back = (self.back + 1) % self.size;
            self.len -= 1;

            if self.len >= 3 {
                // Queue length of 3 or 4 after pop. Set Read delay.
                // This should cover 8088 and 8086(?)
                self.delay = QueueDelay::Read;
            }
            else {
                self.delay = QueueDelay::None;
            }

            return byte
        }
        panic!("Queue underrun!");
    }

    /// Get the active bus delay type based on the last queue operation.
    /// Delay Write is set when the queue length is 3 (or 4 on 8086) and the last operation was a push.
    /// Delay Read is set when the queue length is 3 (or 4 on 8086) and the last operation was a pop.
    /// Delay None is set if neither of these conditions apply.
    #[inline]
    pub fn get_delay(&self) -> QueueDelay {
        self.delay
    }

    /// Flush the processor queue. This resets the queue to an empty state
    /// with no delay flags. 
    pub fn flush(&mut self) {
        self.len = 0;
        self.back = 0;
        self.front = 0;
        self.preload = None;
        self.delay = QueueDelay::None;
    }

    /// Convert the contents of the processor instruction queue to a hexadecimal string.
    pub fn to_string(&self) -> String {

        let mut base_str = "".to_string();

        for i in 0..self.len {
            base_str.push_str(&format!("{:02X}", self.q[(self.back + i) % self.size]));
        }

        base_str
    }

    /// Write the contents of the processor instruction queue in order to the
    /// provided slice of u8. The slice must be the same size as the current piq 
    /// length for the given cpu type.
    #[allow(dead_code)]
    pub fn to_slice(&self, slice: &mut [u8]) {

        assert_eq!(self.size, slice.len());

        for i in 0..self.len {
            slice[i] = self.q[(self.back + i) % self.size];
        }
    }
}