use super::*;
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};

#[test]
fn shared_identification_deadline_stops_before_another_baud_probe() {
    let (io, control) = TestReceiver::new("ZED-F9P");
    control.state.lock().expect("state").silent_identity = true;
    let mut channel = Channel::new(Box::new(io), Cancellation::new());
    assert!(matches!(
        channel.identify_until(0, Instant::now()),
        Err(BaseError::Timeout { .. })
    ));
    assert!(control.state.lock().expect("state").writes.is_empty());
    let started = Instant::now();
    assert!(matches!(
        channel.identify_until(0, started + Duration::from_millis(25)),
        Err(BaseError::Timeout { .. })
    ));
    assert!(started.elapsed() < Duration::from_millis(250));
    let state = control.state.lock().expect("state");
    assert!(
        state.writes.len() <= 1,
        "one shared deadline, not seven fresh budgets"
    );
    assert!(state
        .writes
        .iter()
        .all(|bytes| bytes[2..4] == [MON_VER.0, MON_VER.1]));
}

#[derive(Default)]
pub(crate) struct DeviceState {
    pub writes: Vec<Vec<u8>>,
    pub values: HashMap<u32, u32>,
    pub reject_config: bool,
    pub omit_ack: bool,
    pub silent_identity: bool,
    pub corrupt_readback: bool,
    pub baud_rates: Vec<u32>,
}

pub(crate) struct TestReceiver {
    state: Arc<Mutex<DeviceState>>,
    input: mpsc::Receiver<Vec<u8>>,
    output: mpsc::Sender<Vec<u8>>,
    pending: VecDeque<u8>,
    identity: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct ReceiverControl {
    pub state: Arc<Mutex<DeviceState>>,
    output: mpsc::Sender<Vec<u8>>,
}

impl TestReceiver {
    pub(crate) fn new(model: &str) -> (Self, ReceiverControl) {
        let (output, input) = mpsc::channel();
        let state = Arc::new(Mutex::new(DeviceState::default()));
        let control = ReceiverControl {
            state: state.clone(),
            output: output.clone(),
        };
        (
            Self {
                state,
                input,
                output,
                pending: VecDeque::new(),
                identity: identity(model),
            },
            control,
        )
    }
}

fn identity(model: &str) -> Vec<u8> {
    let mut bytes = vec![0; 40];
    bytes[..8].copy_from_slice(b"EXT CORE");
    bytes[30..38].copy_from_slice(b"00190000");
    for extension in [
        format!("MOD={model}"),
        "FWVER=HPG 1.32".into(),
        "PROTVER=27.31".into(),
    ] {
        let mut block = [0; 30];
        block[..extension.len()].copy_from_slice(extension.as_bytes());
        bytes.extend_from_slice(&block);
    }
    bytes
}

pub(crate) fn survey_payload(duration: u32, accuracy: u32, valid: bool, active: bool) -> Vec<u8> {
    let mut payload = vec![0; 40];
    payload[8..12].copy_from_slice(&duration.to_le_bytes());
    payload[28..32].copy_from_slice(&accuracy.to_le_bytes());
    payload[32..36].copy_from_slice(&(duration * 5).to_le_bytes());
    payload[36] = u8::from(valid);
    payload[37] = u8::from(active);
    payload
}

impl ReceiverControl {
    pub(crate) fn bytes(&self, bytes: Vec<u8>) {
        self.output.send(bytes).expect("receiver still open");
    }

    pub(crate) fn survey(&self, duration: u32, accuracy: u32, valid: bool, active: bool) {
        self.bytes(
            encode(NAV_SVIN, &survey_payload(duration, accuracy, valid, active))
                .expect("survey frame"),
        );
    }

    pub(crate) fn rtcm_enabled(&self) -> bool {
        self.state.lock().expect("state").values.get(&0x2091_02c0) == Some(&1)
    }
}

impl Read for TestReceiver {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            match self.input.recv_timeout(Duration::from_millis(5)) {
                Ok(bytes) => self.pending.extend(bytes),
                Err(_) => return Err(std::io::ErrorKind::TimedOut.into()),
            }
        }
        // Fragment even headers and ACKs: serial reads have no message boundary.
        let count = bytes.len().min(self.pending.len()).min(7);
        for byte in &mut bytes[..count] {
            *byte = self.pending.pop_front().expect("available byte");
        }
        Ok(count)
    }
}

impl Write for TestReceiver {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        assert_eq!(&bytes[..2], &[0xb5, 0x62]);
        let id = (bytes[2], bytes[3]);
        let payload = &bytes[6..bytes.len() - 2];
        let mut state = self.state.lock().expect("state");
        state.writes.push(bytes.to_vec());
        let reply = match id {
            (0x0a, 0x04) if !state.silent_identity => {
                Some(encode(id, &self.identity).expect("identity"))
            }
            (0x06, 0x8a) => {
                assert_eq!(&payload[..4], &[0, 1, 0, 0], "every write must be RAM-only");
                if state.reject_config {
                    Some(encode((5, 0), &[6, 0x8a]).expect("NAK"))
                } else {
                    let mut at = 4;
                    while at < payload.len() {
                        let key = u32::from_le_bytes(payload[at..at + 4].try_into().expect("key"));
                        let width = match key >> 28 {
                            1 | 2 => 1,
                            3 => 2,
                            4 => 4,
                            _ => panic!("unsupported fixture key"),
                        };
                        let mut value = [0; 4];
                        value[..width].copy_from_slice(&payload[at + 4..at + 4 + width]);
                        state.values.insert(key, u32::from_le_bytes(value));
                        at += 4 + width;
                    }
                    (!state.omit_ack).then(|| encode((5, 1), &[6, 0x8a]).expect("ACK"))
                }
            }
            (0x06, 0x8b) => {
                assert_eq!(&payload[..4], &[0, 0, 0, 0], "RAM readback");
                let mut response = vec![1, 0, 0, 0];
                for key in payload[4..].as_chunks::<4>().0 {
                    let key_id = u32::from_le_bytes(*key);
                    let width = match key_id >> 28 {
                        1 | 2 => 1,
                        3 => 2,
                        4 => 4,
                        _ => panic!("key"),
                    };
                    let value = state.values.get(&key_id).copied().unwrap_or(0)
                        ^ u32::from(state.corrupt_readback);
                    response.extend_from_slice(key);
                    response.extend_from_slice(&value.to_le_bytes()[..width]);
                }
                Some(encode(id, &response).expect("readback"))
            }
            (0x01, 0x3b) => Some(encode(id, &survey_payload(0, 0, false, false)).expect("stopped")),
            _ => None,
        };
        if let Some(reply) = reply {
            self.output.send(reply).expect("receiver input");
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl ReceiverIo for TestReceiver {
    fn set_baud_rate(&mut self, baud: u32) -> Result<(), SourceError> {
        self.state.lock().expect("state").baud_rates.push(baud);
        Ok(())
    }
}

#[test]
fn default_survey_has_independently_pinned_keys_units_and_ram_layer() {
    assert_eq!(
        encode(MON_VER, &[]).expect("poll"),
        [0xb5, 0x62, 0x0a, 4, 0, 0, 0x0e, 0x34]
    );
    let values = survey_values(SurveyInConfig::default()).expect("valid defaults");
    assert_eq!(
        valset(&values).expect("payload"),
        [
            0, 1, 0, 0, 1, 0, 3, 0x20, 1, // TMODE mode 1
            0x10, 0, 3, 0x40, 180, 0, 0, 0, // seconds
            0x11, 0, 3, 0x40, 0x20, 0x4e, 0, 0, // 20000 x 0.1 mm
            0x89, 0, 0x91, 0x20, 5, // NAV-SVIN UART1
            0x8b, 0, 0x91, 0x20, 5, // NAV-SVIN USB
        ]
    );
}

#[test]
fn survey_report_uses_wire_offsets_and_rejects_wrong_shapes() {
    let bytes = survey_payload(181, 12_345, true, false);
    let status = survey_status(&bytes).expect("report");
    assert_eq!(status.duration_s, 181);
    assert_eq!(status.observations, 905);
    assert_eq!(status.mean_accuracy_m, 1.2345);
    assert!(status.valid && !status.active);
    assert!(survey_status(&bytes[..39]).is_none());
    for offset in [0, 36, 37] {
        let mut invalid = bytes.clone();
        invalid[offset] = 2;
        assert!(survey_status(&invalid).is_none());
    }
}

#[test]
fn mixed_stream_handles_splits_noise_crc_errors_and_nested_rtcm() {
    let rtcm = rtcm::encode_frame(&[0x3e, 0xd0, 1, 2]).expect("RTCM 1005");
    let wrapped = encode((0x0a, 0x09), &rtcm).expect("UBX containing RTCM bytes");
    let mut bad = wrapped.clone();
    *bad.last_mut().expect("checksum") ^= 0x40;
    let stream = [
        vec![1, 2, 0xb5, 7],
        wrapped,
        rtcm.clone(),
        bad,
        rtcm.clone(),
    ]
    .concat();
    let mut framer = ReceiverFramer::default();
    let frames: Vec<_> = stream.iter().flat_map(|b| framer.push(&[*b])).collect();
    assert!(matches!(&frames[0], WireFrame::Ubx(_)));
    // The valid outer UBX must not leak its embedded correction. A corrupt
    // outer frame is resynchronised, so its intact inner RTCM can be recovered.
    assert_eq!(
        frames
            .iter()
            .filter(|f| matches!(f, WireFrame::Rtcm(_)))
            .count(),
        3
    );
    assert!(framer.bytes.is_empty());
    let mut oversized = vec![0xb5, 0x62, 1, 1, 0xff, 0xff];
    oversized.extend_from_slice(&rtcm);
    assert_eq!(framer.push(&oversized).len(), 1);
}

#[test]
fn readback_checks_keys_values_duplicates_and_layer() {
    let expected = [(0x4003_0010, 180)];
    let good = [1, 0, 0, 0, 0x10, 0, 3, 0x40, 180, 0, 0, 0];
    verify_readback(&good, &expected).expect("RAM matches");
    let mut wrong = good;
    wrong[8] = 179;
    assert!(verify_readback(&wrong, &expected).is_err());
    wrong = good;
    wrong[1] = 1;
    assert!(verify_readback(&wrong, &expected).is_err());
    assert!(verify_readback(&good[..4], &expected).is_err());
    assert!(verify_readback(&[good.as_slice(), &good[4..]].concat(), &expected).is_err());
}

#[test]
fn f9p_identification_rejects_other_ublox_models_and_missing_identity() {
    assert!(f9p_identity(&identity("ZED-F9P")).is_ok());
    for model in ["ZED-F9R", "NEO-M9N", "NEO-M8P", "unknown"] {
        assert!(matches!(
            f9p_identity(&identity(model)),
            Err(BaseError::UnsupportedReceiver)
        ));
    }
    assert!(f9p_identity(&identity("ZED-F9P")[..40]).is_err());
}

#[test]
fn configuration_stops_old_survey_and_verifies_every_ram_write() {
    let (io, control) = TestReceiver::new("ZED-F9P");
    let mut channel = Channel::new(Box::new(io), Cancellation::new());
    channel.identify(115_200).expect("F9P");
    channel
        .configure(SurveyInConfig::default(), 115_200)
        .expect("configured");
    assert!(!control.rtcm_enabled());
    {
        let state = control.state.lock().expect("state");
        assert_eq!(state.values.get(&0x4003_0010), Some(&180));
        assert_eq!(state.values.get(&0x4003_0011), Some(&20_000));
        let stop = state
            .writes
            .iter()
            .position(|bytes| bytes[2..4] == [1, 0x3b])
            .expect("survey stop poll");
        let enable = state
            .writes
            .iter()
            .rposition(|bytes| bytes[2..4] == [6, 0x8a])
            .expect("enable survey");
        assert!(stop < enable);
        for (index, bytes) in state.writes.iter().enumerate() {
            if bytes[2..4] == [6, 0x8a] {
                assert_eq!(&state.writes[index + 1][2..4], &[6, 0x8b]);
            }
        }
    }
    channel.activate_rtcm().expect("activated");
    let state = control.state.lock().expect("state");
    for key in [
        0x2091_02c0,
        0x2091_02cf,
        0x2091_02d4,
        0x2091_031b,
        0x2091_02d9,
        0x2091_0306,
    ] {
        assert_eq!(state.values.get(&key), Some(&1), "1005/MSM7/1230 on USB");
    }
    assert_eq!(state.values.get(&0x3021_0001), Some(&1000));
}

#[test]
fn required_nak_timeout_and_bad_readback_end_configuration() {
    for failure in 0..3 {
        let (io, control) = TestReceiver::new("ZED-F9P");
        let mut channel = Channel::new(Box::new(io), Cancellation::new());
        channel.identify(115_200).expect("identify");
        {
            let mut state = control.state.lock().expect("state");
            state.reject_config = failure == 0;
            state.omit_ack = failure == 1;
            state.corrupt_readback = failure == 2;
        }
        let error = channel
            .configure(SurveyInConfig::default(), 115_200)
            .expect_err("must fail");
        assert!(matches!(
            error,
            BaseError::Rejected { .. }
                | BaseError::Timeout { .. }
                | BaseError::ReadbackMismatch { .. }
        ));
        let state = control.state.lock().expect("state");
        assert_eq!(
            state.writes.iter().filter(|b| b[2..4] == [6, 0x8a]).count(),
            1,
            "no later writes after ambiguity/failure"
        );
    }
}

#[test]
fn automatic_baud_change_requires_identity_and_readback_at_new_rate() {
    let (io, control) = TestReceiver::new("ZED-F9P");
    let mut channel = Channel::new(Box::new(io), Cancellation::new());
    channel.identify(0).expect("identify");
    channel
        .configure(SurveyInConfig::default(), 0)
        .expect("configure");
    let state = control.state.lock().expect("state");
    assert_eq!(state.baud_rates, [38_400, 115_200]);
    assert_eq!(state.values.get(&0x4052_0001), Some(&115_200));
    assert_eq!(
        state.writes.iter().filter(|b| b[2..4] == [0x0a, 4]).count(),
        2
    );
}
