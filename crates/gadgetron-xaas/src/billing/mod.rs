//! Billing module — integer-cent ledger writer (ISSUE 12 TASK 12.1).
//!
//! Thin helpers that persist one `billing_events` row per billable
//! event. Called fire-and-forget from the quota path's `record_post`
//! hook so failures surface as tracing warnings without blocking
//! the request.

pub mod events;

pub use events::{insert_billing_event, BillingEventInsert, BillingEventKind, BillingEventRow};
