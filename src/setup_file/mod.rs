//! The setup file: an encrypted file that carries an organization's sign-in
//! settings (client ID, tenant, optional relay) to a new install.

pub mod file;
pub mod format;
pub mod kdf;
pub mod passphrase;
pub mod payload;

pub use file::{open_file, seal_file};
pub use format::SetupError;
pub use kdf::{DEFAULT_KDF, KdfParams};
pub use payload::{SetupPayload, SetupSummary};
