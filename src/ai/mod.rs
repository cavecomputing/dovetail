//! The assistant pane's logic: talking to any OpenAI-compatible server, the tools the model may
//! call, and the conversation loop. Nothing in here imports iced, so all of it can be tested
//! without a window.

pub mod agent;
pub mod client;
pub mod context;
pub mod key;
pub mod sse;
pub mod tools;
