//! SSH authentication mode placeholders.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    Password,
    PrivateKey { key_path: String },
    Agent,
    KeyboardInteractive,
}
