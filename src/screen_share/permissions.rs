//! Permission policy boundary for screen capture and remote input.

/// Placeholder permission state used until platform permission integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    /// Permission has not been requested.
    Unknown,
    /// Permission was granted.
    Granted,
    /// Permission was denied.
    Denied,
}
