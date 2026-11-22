//! What an external `agents` plugin may read and propose, per account.
//!
//! Pure state and helpers: the allow list (`access`), the plain message view
//! handed to agents (`view`) and pending reply and reaction proposals
//! (`proposals`). Nothing here touches the network, and no agent path
//! fetches from Graph.

pub mod access;
pub mod proposals;
pub mod view;
