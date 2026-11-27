pub const REQUEST_MAGIC: [u8; 8] = *b"GNLP1REQ";
pub const RESPONSE_OK: [u8; 8] = *b"GNLP1OK!";
pub const RESPONSE_DENY: [u8; 8] = *b"GNLP1DEN";
pub const RESPONSE_ERROR: [u8; 8] = *b"GNLP1ERR";
pub const MAX_PASSWORD_BYTES: usize = 1024;
