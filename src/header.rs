pub const HEADER_LENGTH: usize = 10;

#[derive(Debug, PartialEq)]
#[repr(u8)]
pub(crate) enum Flag {
    Syn = 0x01,
    Ack,
    Fin,
}

impl From<u8> for Flag {
    fn from(v: u8) -> Self {
        match v {
            0x01 => Flag::Syn,
            0x02 => Flag::Ack,
            _ => Flag::Fin,
        }
    }
}

impl Into<u8> for Flag {
    fn into(self) -> u8 {
        self as u8
    }
}

#[derive(Debug)]
#[repr(u8)]
pub(crate) enum FrameType {
    Ping = 0x01,
    Close,
    Data,
}

impl From<u8> for FrameType {
    fn from(v: u8) -> Self {
        match v {
            0x01 => FrameType::Ping,
            0x02 => FrameType::Close,
            _ => FrameType::Data,
        }
    }
}

impl Into<u8> for FrameType {
    fn into(self) -> u8 {
        self as u8
    }
}

#[derive(Debug)]
pub struct MuxHeader {
    stream_id: u32,
    length: u32,
    flag: Flag,
    frame_type: FrameType,
}

impl MuxHeader {
    pub(crate) fn new(
        id: u32,
        length: u32,
        flag: Option<Flag>,
        frame_type: Option<FrameType>,
    ) -> Self {
        MuxHeader {
            stream_id: id,
            length: length,
            flag: flag.unwrap_or(Flag::Ack),
            frame_type: frame_type.unwrap_or(FrameType::Data),
        }
    }

    pub(crate) fn from_bytes(bytes: [u8; HEADER_LENGTH]) -> Self {
        MuxHeader {
            stream_id: u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            length: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            flag: Flag::from(bytes[8]),
            frame_type: FrameType::from(bytes[9]),
        }
    }

    pub(crate) fn to_bytes(self) -> [u8; HEADER_LENGTH] {
        let mut data: [u8; HEADER_LENGTH] = [0; HEADER_LENGTH];
        data[..4].copy_from_slice(&self.stream_id.to_be_bytes());
        data[4..8].copy_from_slice(&self.length.to_be_bytes());
        data[8] = self.flag.into();
        data[9] = self.frame_type.into();
        data
    }

    pub(crate) fn len(&self) -> usize {
        self.length as usize
    }

    pub(crate) fn id(&self) -> u32 {
        self.stream_id
    }

    pub(crate) fn set_flag(&mut self, flag: Flag) {
        self.flag = flag;
    }

    pub(crate) fn flag(&self) -> &Flag {
        &self.flag
    }

    pub(crate) fn frame_type(&self) -> &FrameType {
        &self.frame_type
    }
}
