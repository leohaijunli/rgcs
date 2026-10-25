//! F9P UBX configuration and mixed UBX/RTCM framing.
//!
//! Wire reference: UBX-22008968 R01, sections 3.10.25/26, 3.15.21 and
//! 6.9.28. Configuration sequencing follows QGC 73e64bc81b's UBX driver.
//! Every required VALSET is followed by a RAM VALGET; no nonvolatile layer
//! is selected. Only MON-VER polls are sent until a supported F9P is found.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use super::base::{BaseError, Cancellation, SurveyInConfig, SurveyInStatus, ACCURACY_UNITS_PER_M};
use super::rtcm::{self, RtcmError, RtcmFrame};
use super::source::SourceError;

pub(crate) const BAUD_RATES: [u32; 7] = [38_400, 57_600, 9_600, 115_200, 230_400, 460_800, 921_600];
const BASE_BAUD: u32 = 115_200;
const IDENTITY_TIMEOUT: Duration = Duration::from_secs(2);
const CONFIG_TIMEOUT: Duration = Duration::from_millis(250);
const SURVEY_STOP_TIMEOUT: Duration = Duration::from_secs(3);
const IDLE_SLEEP: Duration = Duration::from_millis(5);
const MAX_UBX_PAYLOAD: usize = 4096;
const UBX_HEADER: usize = 6;
const UBX_OVERHEAD: usize = 8;
const MON_VER_HEADER: usize = 40;
const MON_VER_EXTENSION: usize = 30;
const NAV_SVIN_LEN: usize = 40;
const SURVEY_PERIOD_MS: u32 = 200;
const BASE_PERIOD_MS: u32 = 1000;
const SURVEY_STATUS_DIVISOR: u32 = 5;
const PORT_OFFSETS: [u32; 2] = [1, 3]; // UART1 and USB, relative to the I2C MSGOUT key.
const MAX_CONFIG_KEYS: usize = 64;

const MON_VER: (u8, u8) = (0x0a, 0x04);
pub(crate) const NAV_SVIN: (u8, u8) = (0x01, 0x3b);
const CFG_VALSET: (u8, u8) = (0x06, 0x8a);
const CFG_VALGET: (u8, u8) = (0x06, 0x8b);
const ACK_ACK: (u8, u8) = (0x05, 0x01);
const ACK_NAK: (u8, u8) = (0x05, 0x00);

const TMODE_MODE: u32 = 0x2003_0001;
const TMODE_MIN_DUR: u32 = 0x4003_0010;
const TMODE_ACC_LIMIT: u32 = 0x4003_0011;
const UART1_BAUD: u32 = 0x4052_0001;
const RATE_MEAS: u32 = 0x3021_0001;
const RATE_NAV: u32 = 0x3021_0002;
const RATE_TIMEREF: u32 = 0x2021_0003;
const NAV_DYNMODEL: u32 = 0x2011_0021;
const NAV_FIXMODE: u32 = 0x2011_0011;
const NAV_SVIN_OUT: u32 = 0x2091_0088;
const RTCM1005: u32 = 0x2091_02bd;
const RTCM1230: u32 = 0x2091_0303;
const MSM7: [u32; 4] = [0x2091_02cc, 0x2091_02d1, 0x2091_0318, 0x2091_02d6];
const MSM4: [u32; 4] = [0x2091_035e, 0x2091_0363, 0x2091_0368, 0x2091_036d];
const PORT_PROTOCOLS: [(u32, u32); 10] = [
    (0x1073_0001, 1), // UART1 UBX input
    (0x1074_0001, 1), // UART1 UBX output
    (0x1074_0002, 0), // UART1 NMEA output
    (0x1074_0004, 1), // UART1 RTCM output
    (0x1073_0004, 0), // UART1 RTCM input
    (0x1077_0001, 1), // USB UBX input
    (0x1078_0001, 1), // USB UBX output
    (0x1078_0002, 1), // USB NMEA output
    (0x1078_0004, 1), // USB RTCM output
    (0x1077_0004, 0), // USB RTCM input
];

/// Reads and writes must return within 100 ms, including timeout errors.
pub(crate) trait ReceiverIo: Read + Write + Send {
    /// Changes the host's line rate, not the receiver's configuration.
    fn set_baud_rate(&mut self, baud: u32) -> Result<(), SourceError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UbxMessage {
    pub class: u8,
    pub id: u8,
    pub payload: Vec<u8>,
}

impl UbxMessage {
    pub fn is(&self, id: (u8, u8)) -> bool {
        (self.class, self.id) == id
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WireFrame {
    Ubx(UbxMessage),
    Rtcm(RtcmFrame),
}

fn checksum(bytes: &[u8]) -> [u8; 2] {
    let mut a = 0u8;
    let mut b = 0u8;
    for byte in bytes {
        a = a.wrapping_add(*byte);
        b = b.wrapping_add(a);
    }
    [a, b]
}

fn encode(id: (u8, u8), payload: &[u8]) -> Result<Vec<u8>, BaseError> {
    if payload.len() > MAX_UBX_PAYLOAD {
        return Err(BaseError::InvalidConfig {
            field: "UBX payload is too large",
        });
    }
    let mut bytes = Vec::with_capacity(payload.len() + UBX_OVERHEAD);
    bytes.extend_from_slice(&[0xb5, 0x62, id.0, id.1]);
    bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&checksum(&bytes[2..]));
    Ok(bytes)
}

/// A valid UBX payload may itself contain a complete RTCM frame. Consume the
/// outer frame atomically, rather than feeding both independent framers.
#[derive(Default)]
pub(crate) struct ReceiverFramer {
    bytes: Vec<u8>,
}

impl ReceiverFramer {
    pub fn push(&mut self, input: &[u8]) -> Vec<WireFrame> {
        let mut frames = Vec::new();
        // Bound buffered input even for an arbitrarily large source chunk.
        for chunk in input.chunks(MAX_UBX_PAYLOAD) {
            self.bytes.extend_from_slice(chunk);
            let mut start = 0;
            while start < self.bytes.len() {
                let bytes = &self.bytes[start..];
                match bytes[0] {
                    0xb5 => {
                        if bytes.len() < 2 {
                            break;
                        }
                        if bytes[1] != 0x62 {
                            start += 1;
                            continue;
                        }
                        if bytes.len() < UBX_HEADER {
                            break;
                        }
                        let len = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
                        if len > MAX_UBX_PAYLOAD {
                            start += 1;
                            continue;
                        }
                        let total = len + UBX_OVERHEAD;
                        if bytes.len() < total {
                            break;
                        }
                        if checksum(&bytes[2..total - 2]) != bytes[total - 2..total] {
                            start += 1;
                            continue;
                        }
                        frames.push(WireFrame::Ubx(UbxMessage {
                            class: bytes[2],
                            id: bytes[3],
                            payload: bytes[UBX_HEADER..total - 2].to_vec(),
                        }));
                        start += total;
                    }
                    rtcm::PREAMBLE => match rtcm::parse_frame(bytes) {
                        Ok((frame, consumed)) => {
                            frames.push(WireFrame::Rtcm(frame));
                            start += consumed;
                        }
                        Err(RtcmError::Incomplete) => break,
                        Err(_) => start += 1,
                    },
                    _ => start += 1,
                }
            }
            self.bytes.drain(..start);
        }
        frames
    }
}

fn u32_at(payload: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        payload.get(at..at + 4)?.try_into().ok()?,
    ))
}

pub(crate) fn survey_status(payload: &[u8]) -> Option<SurveyInStatus> {
    if payload.len() != NAV_SVIN_LEN || payload[0] != 0 || payload[36] > 1 || payload[37] > 1 {
        return None;
    }
    Some(SurveyInStatus {
        duration_s: u32_at(payload, 8)?,
        mean_accuracy_m: f64::from(u32_at(payload, 28)?) / ACCURACY_UNITS_PER_M,
        observations: u32_at(payload, 32)?,
        valid: payload[36] == 1,
        active: payload[37] == 1,
    })
}

fn f9p_identity(payload: &[u8]) -> Result<String, BaseError> {
    if payload.len() < MON_VER_HEADER
        || !(payload.len() - MON_VER_HEADER).is_multiple_of(MON_VER_EXTENSION)
    {
        return Err(BaseError::UnsupportedReceiver);
    }
    let mut model = None;
    let mut firmware = None;
    let mut protocol = None;
    for block in payload[MON_VER_HEADER..].as_chunks::<MON_VER_EXTENSION>().0 {
        let end = block.iter().position(|b| *b == 0).unwrap_or(block.len());
        let value =
            std::str::from_utf8(&block[..end]).map_err(|_| BaseError::UnsupportedReceiver)?;
        if let Some(value) = value.strip_prefix("MOD=") {
            model = Some(value);
        }
        if let Some(value) = value.strip_prefix("FWVER=") {
            firmware = Some(value);
        }
        if let Some(value) = value.strip_prefix("PROTVER=") {
            protocol = Some(value);
        }
    }
    let model = model
        .filter(|model| *model == "ZED-F9P" || model.starts_with("ZED-F9P-"))
        .ok_or(BaseError::UnsupportedReceiver)?;
    let protocol = protocol.ok_or(BaseError::UnsupportedReceiver)?;
    let major = protocol
        .split('.')
        .next()
        .and_then(|n| n.parse::<u32>().ok());
    if !major.is_some_and(|major| major >= 27)
        || !firmware.is_some_and(|fw| fw.starts_with("HPG ") || fw.starts_with("HPGL1L5 "))
    {
        return Err(BaseError::UnsupportedReceiver);
    }
    Ok(format!(
        "{model} · {} · UBX {protocol}",
        firmware.unwrap_or_default()
    ))
}

fn value_width(key: u32) -> Option<usize> {
    match (key >> 28) & 7 {
        1 | 2 => Some(1),
        3 => Some(2),
        4 => Some(4),
        _ => None,
    }
}

fn valset(values: &[(u32, u32)]) -> Result<Vec<u8>, BaseError> {
    if values.is_empty() || values.len() > MAX_CONFIG_KEYS {
        return Err(BaseError::InvalidConfig {
            field: "invalid UBX configuration batch size",
        });
    }
    let mut payload = vec![0, 1, 0, 0]; // Version 0, RAM only, reserved bytes.
    for &(key, value) in values {
        let width = value_width(key).ok_or(BaseError::InvalidConfig {
            field: "invalid UBX configuration key",
        })?;
        if width < 4 && value >= (1 << (width * 8)) || key >> 28 == 1 && value > 1 {
            return Err(BaseError::InvalidConfig {
                field: "UBX configuration value is out of range",
            });
        }
        payload.extend_from_slice(&key.to_le_bytes());
        payload.extend_from_slice(&value.to_le_bytes()[..width]);
    }
    Ok(payload)
}

fn verify_readback(payload: &[u8], expected: &[(u32, u32)]) -> Result<(), BaseError> {
    let mismatch = || BaseError::ReadbackMismatch {
        key: expected.first().map(|v| v.0).unwrap_or(0),
    };
    if payload.get(..4) != Some(&[1, 0, 0, 0]) {
        return Err(mismatch());
    }
    let mut remaining = &payload[4..];
    let mut seen = Vec::new();
    while !remaining.is_empty() {
        let key = u32_at(remaining, 0).ok_or_else(mismatch)?;
        let width = value_width(key).ok_or_else(mismatch)?;
        let bytes = remaining.get(4..4 + width).ok_or_else(mismatch)?;
        let mut value = [0; 4];
        value[..width].copy_from_slice(bytes);
        if !expected.contains(&(key, u32::from_le_bytes(value))) || seen.contains(&key) {
            return Err(BaseError::ReadbackMismatch { key });
        }
        seen.push(key);
        remaining = &remaining[4 + width..];
    }
    for &(key, _) in expected {
        if !seen.contains(&key) {
            return Err(BaseError::ReadbackMismatch { key });
        }
    }
    Ok(())
}

fn outputs(keys: impl IntoIterator<Item = u32>, rate: u32) -> Vec<(u32, u32)> {
    keys.into_iter()
        .flat_map(|key| PORT_OFFSETS.map(|port| (key + port, rate)))
        .collect()
}

fn survey_values(config: SurveyInConfig) -> Result<Vec<(u32, u32)>, BaseError> {
    config.validate()?;
    let mut values = vec![
        (TMODE_MODE, 1),
        (TMODE_MIN_DUR, config.min_duration_s),
        (TMODE_ACC_LIMIT, config.wire_accuracy_limit()?),
    ];
    values.extend(outputs([NAV_SVIN_OUT], SURVEY_STATUS_DIVISOR));
    Ok(values)
}

pub(crate) struct Channel {
    io: Box<dyn ReceiverIo>,
    cancel: Cancellation,
    framer: ReceiverFramer,
    pending: VecDeque<WireFrame>,
    last_data: Option<Instant>,
    detected_baud: u32,
    identity: String,
}

impl Channel {
    pub fn new(io: Box<dyn ReceiverIo>, cancel: Cancellation) -> Self {
        Self {
            io,
            cancel,
            framer: ReceiverFramer::default(),
            pending: VecDeque::new(),
            last_data: None,
            detected_baud: 0,
            identity: String::new(),
        }
    }

    pub fn last_data(&self) -> Option<Instant> {
        self.last_data
    }

    pub fn receive(&mut self) -> Result<Vec<WireFrame>, BaseError> {
        self.cancel.check()?;
        if !self.pending.is_empty() {
            return Ok(self.pending.drain(..).collect());
        }
        let mut bytes = [0; 2048];
        let len = match self.io.read(&mut bytes) {
            Ok(len) => len,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                0
            }
            Err(error) => return Err(SourceError::Io(error).into()),
        };
        self.cancel.check()?;
        if len > 0 {
            self.last_data = Some(Instant::now());
        } else {
            std::thread::sleep(IDLE_SLEEP);
        }
        Ok(self.framer.push(&bytes[..len]))
    }

    fn write(
        &mut self,
        id: (u8, u8),
        payload: &[u8],
        deadline: Instant,
        stage: &'static str,
    ) -> Result<(), BaseError> {
        let bytes = encode(id, payload)?;
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            self.cancel.check()?;
            if Instant::now() >= deadline {
                return Err(BaseError::Timeout { stage });
            }
            match self.io.write(remaining) {
                Ok(0) => return Err(SourceError::Io(std::io::ErrorKind::WriteZero.into()).into()),
                Ok(len) => remaining = &remaining[len..],
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    std::thread::sleep(IDLE_SLEEP)
                }
                Err(error) => return Err(SourceError::Io(error).into()),
            }
        }
        self.cancel.check()
    }

    fn await_message(
        &mut self,
        id: (u8, u8),
        deadline: Instant,
        stage: &'static str,
    ) -> Result<UbxMessage, BaseError> {
        loop {
            self.cancel.check()?;
            if Instant::now() >= deadline {
                return Err(BaseError::Timeout { stage });
            }
            if let Some(frame) = self.pending.pop_front() {
                if let WireFrame::Ubx(message) = frame {
                    if message.is(ACK_NAK) && message.payload == [id.0, id.1] {
                        return Err(BaseError::Rejected {
                            class: id.0,
                            id: id.1,
                        });
                    }
                    if message.is(id) {
                        return Ok(message);
                    }
                }
            } else {
                let frames = self.receive()?;
                self.pending.extend(frames);
            }
        }
    }

    fn poll(
        &mut self,
        id: (u8, u8),
        payload: &[u8],
        timeout: Duration,
        stage: &'static str,
    ) -> Result<UbxMessage, BaseError> {
        let deadline = Instant::now() + timeout;
        self.write(id, payload, deadline, stage)?;
        self.await_message(id, deadline, stage)
    }

    #[cfg(test)]
    pub fn identify(&mut self, requested_baud: u32) -> Result<String, BaseError> {
        self.identify_until(
            requested_baud,
            Instant::now() + super::base::BASE_IDENTIFICATION_TIMEOUT,
        )
    }

    pub fn identify_until(
        &mut self,
        requested_baud: u32,
        deadline: Instant,
    ) -> Result<String, BaseError> {
        let bauds = if requested_baud == 0 {
            BAUD_RATES.to_vec()
        } else {
            vec![requested_baud]
        };
        for baud in bauds {
            self.cancel.check()?;
            if Instant::now() >= deadline {
                return Err(BaseError::Timeout {
                    stage: "identifying F9P within startup budget",
                });
            }
            self.io.set_baud_rate(baud)?;
            self.pending.clear();
            self.framer = ReceiverFramer::default();
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(IDENTITY_TIMEOUT);
            match self.poll(MON_VER, &[], remaining, "identifying F9P (MON-VER)") {
                Ok(reply) => {
                    let identity = f9p_identity(&reply.payload)?;
                    self.detected_baud = baud;
                    self.identity = identity.clone();
                    return Ok(identity);
                }
                Err(BaseError::Timeout { .. }) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(BaseError::Timeout {
            stage: "identifying F9P (check receiver and baud rate)",
        })
    }

    fn readback(&mut self, values: &[(u32, u32)]) -> Result<(), BaseError> {
        let mut payload = vec![0; 4]; // VALGET version 0, RAM layer 0, position 0.
        for &(key, _) in values {
            payload.extend_from_slice(&key.to_le_bytes());
        }
        let reply = self.poll(
            CFG_VALGET,
            &payload,
            CONFIG_TIMEOUT,
            "configuration readback",
        )?;
        verify_readback(&reply.payload, values)
    }

    fn set(&mut self, values: &[(u32, u32)]) -> Result<(), BaseError> {
        let deadline = Instant::now() + CONFIG_TIMEOUT;
        self.write(
            CFG_VALSET,
            &valset(values)?,
            deadline,
            "writing receiver configuration",
        )?;
        loop {
            let reply = self.await_ack(deadline)?;
            if reply.payload == [CFG_VALSET.0, CFG_VALSET.1] {
                if reply.is(ACK_NAK) {
                    return Err(BaseError::Rejected {
                        class: CFG_VALSET.0,
                        id: CFG_VALSET.1,
                    });
                }
                break;
            }
        }
        self.readback(values)
    }

    fn await_ack(&mut self, deadline: Instant) -> Result<UbxMessage, BaseError> {
        loop {
            self.cancel.check()?;
            if Instant::now() >= deadline {
                return Err(BaseError::Timeout {
                    stage: "configuration acknowledgement",
                });
            }
            if let Some(frame) = self.pending.pop_front() {
                if let WireFrame::Ubx(reply) = frame {
                    if reply.is(ACK_ACK) || reply.is(ACK_NAK) {
                        return Ok(reply);
                    }
                }
            } else {
                let frames = self.receive()?;
                self.pending.extend(frames);
            }
        }
    }

    pub fn configure(
        &mut self,
        config: SurveyInConfig,
        requested_baud: u32,
    ) -> Result<(), BaseError> {
        config.validate()?;
        self.set(&PORT_PROTOCOLS)?;
        let desired_baud = if requested_baud == 0 {
            BASE_BAUD
        } else {
            requested_baud
        };
        if desired_baud != self.detected_baud {
            // The baud-change ACK can be lost during UART handoff. Establish a
            // MON-VER barrier at the new rate, then verify the exact RAM value.
            self.write(
                CFG_VALSET,
                &valset(&[(UART1_BAUD, desired_baud)])?,
                Instant::now() + CONFIG_TIMEOUT,
                "changing receiver baud rate",
            )?;
            self.io.set_baud_rate(desired_baud)?;
            self.pending.clear();
            self.framer = ReceiverFramer::default();
            let identity = self.poll(
                MON_VER,
                &[],
                IDENTITY_TIMEOUT,
                "confirming receiver after baud change",
            )?;
            if f9p_identity(&identity.payload)? != self.identity {
                return Err(BaseError::UnsupportedReceiver);
            }
            self.readback(&[(UART1_BAUD, desired_baud)])?;
        }
        self.set(&[
            (RATE_MEAS, SURVEY_PERIOD_MS),
            (RATE_NAV, 1),
            (RATE_TIMEREF, 0),
            (NAV_DYNMODEL, 2),
            (NAV_FIXMODE, 3),
        ])?;
        self.set(&outputs(
            [RTCM1005, RTCM1230].into_iter().chain(MSM7).chain(MSM4),
            0,
        ))?;
        self.set(&[(TMODE_MODE, 0)])?;
        let deadline = Instant::now() + SURVEY_STOP_TIMEOUT;
        loop {
            self.cancel.check()?;
            if Instant::now() >= deadline {
                return Err(BaseError::Timeout {
                    stage: "stopping the previous survey",
                });
            }
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(CONFIG_TIMEOUT);
            let reply = self.poll(NAV_SVIN, &[], remaining, "confirming stopped survey")?;
            if survey_status(&reply.payload).is_some_and(|status| !status.active && !status.valid) {
                break;
            }
            std::thread::sleep(IDLE_SLEEP);
        }
        self.set(&survey_values(config)?)
    }

    pub fn activate_rtcm(&mut self) -> Result<(), BaseError> {
        let mut values = vec![(RATE_MEAS, BASE_PERIOD_MS)];
        values.extend(outputs([RTCM1005, RTCM1230].into_iter().chain(MSM7), 1));
        values.extend(outputs(MSM4, 0));
        // Keep a 1 Hz survey report so receiver resets/mode changes invalidate
        // readiness. QGC disables this report once activation has succeeded.
        values.extend(outputs([NAV_SVIN_OUT], 1));
        self.set(&values)
    }
}

#[cfg(test)]
pub(crate) mod tests;
