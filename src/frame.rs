use std::fmt::Debug;

use crate::header::{Flag, MuxHeader};

pub struct Frame {
    pub header: MuxHeader,
    pub data: Vec<u8>,
}

impl Frame {
    pub(crate) fn new(header: MuxHeader, data: Vec<u8>) -> Self {
        Frame { header, data }
    }

    pub(crate) fn header(&self) -> &MuxHeader {
        &self.header
    }

    pub fn bytes(&self) -> &Vec<u8> {
        &self.data
    }

    pub(crate) fn split(self) -> (MuxHeader, Vec<u8>) {
        (self.header, self.data)
    }

    pub(crate) fn is_syn(&self) -> bool {
        self.header.flag() == &Flag::Syn
    }

    pub(crate) fn is_ack(&self) -> bool {
        self.header.flag() == &Flag::Ack
    }

    pub fn as_bytes(self) -> Vec<u8> {
        let mut bytes = vec![];
        bytes.extend_from_slice(&self.header.to_bytes());
        bytes.extend_from_slice(&self.data);
        bytes
    }

    pub fn set_flag(&mut self, flag: Flag) {
        self.header.set_flag(flag);
    }
}

impl Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entry(&"stream_id", &self.header.id())
            .entry(&"flag", &self.header.flag())
            .entry(&"type", &self.header.frame_type())
            .entry(&"len", &self.header.len())
            .finish()
    }
}
