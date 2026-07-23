//! Notification system for Boru Chat.
//!
//! This module provides internal notification event types, a notification
//! service interface, platform backend abstraction, and policies for
//! deciding when to show notifications.

pub mod backend;
pub mod event;
pub mod service;
