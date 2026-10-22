//! FC parameter protocol module (parameters plan §4).
//!
//! Pure logic here (`model`/`codec`/`session`/`diff`/`file`) so headless
//! mode reuses it unchanged; the tokio service lives in `service` (PM2) and
//! the shared read cache in `cache`.

pub mod codec;
pub mod diff;
pub mod file;
pub mod model;
pub mod service;
pub mod session;

pub use codec::{decode, decode_param_id, encode, encode_param_id, IntEncoding, PARAM_ID_LEN};
pub use diff::{diff_param_sets, ParamDiff, ParamDiffKind};
pub use file::{export_json, export_qgc_params, import_params_file, ImportPreview};
pub use model::{ParamEntry, ParamType, ParamValue};
pub use service::{ParamCache, ParamError, ParamEvent, ParamObservation, ParamService, SetResult};
