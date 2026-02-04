use std::fmt::Debug;
use thiserror::*;
use tracing::debug;

use super::protocol::*;
use crate::multiplexer;

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("attempted to receive message while agency is ours")]
    AgencyIsOurs,

    #[error("attempted to send message while agency is theirs")]
    AgencyIsTheirs,

    #[error("inbound message is not valid for current state")]
    InvalidInbound,

    #[error("outbound message is not valid for current state")]
    InvalidOutbound,

    #[error("error while sending or receiving data through the channel")]
    Plexer(multiplexer::Error),
}

pub enum ServerState {
    Idle,
    Busy,
    Done,
}

pub struct Server(ServerState, multiplexer::ChannelBuffer);

impl Server {
    pub fn new(channel: multiplexer::AgentChannel) -> Self {
        Self(ServerState::Idle, multiplexer::ChannelBuffer::new(channel))
    }

    pub fn state(&self) -> &ServerState {
        &self.0
    }

    pub fn is_done(&self) -> bool {
        matches!(self.0, ServerState::Done)
    }

    pub fn has_agency(&self) -> bool {
        match &self.0 {
            ServerState::Idle => false,
            ServerState::Busy => true,
            ServerState::Done => false,
        }
    }

    fn assert_agency_is_ours(&self) -> Result<(), ServerError> {
        if !self.has_agency() {
            Err(ServerError::AgencyIsTheirs)
        } else {
            Ok(())
        }
    }

    fn assert_agency_is_theirs(&self) -> Result<(), ServerError> {
        if self.has_agency() {
            Err(ServerError::AgencyIsOurs)
        } else {
            Ok(())
        }
    }

    fn assert_outbound_state(&self, msg: &Message) -> Result<(), ServerError> {
        match (&self.0, msg) {
            (ServerState::Busy, Message::Response(..)) => Ok(()),
            _ => Err(ServerError::InvalidOutbound),
        }
    }

    fn assert_inbound_state(&self, msg: &Message) -> Result<(), ServerError> {
        match (&self.0, msg) {
            (ServerState::Idle, Message::Request(..)) => Ok(()),
            (ServerState::Idle, Message::Done) => Ok(()),
            _ => Err(ServerError::InvalidInbound),
        }
    }

    pub async fn send_message(&mut self, msg: &Message) -> Result<(), ServerError> {
        self.assert_agency_is_ours()?;
        self.assert_outbound_state(msg)?;
        self.1
            .send_msg_chunks(msg)
            .await
            .map_err(ServerError::Plexer)?;

        Ok(())
    }

    pub async fn recv_message(&mut self) -> Result<Message, ServerError> {
        self.assert_agency_is_theirs()?;
        let msg = self.1.recv_full_msg().await.map_err(ServerError::Plexer)?;
        self.assert_inbound_state(&msg)?;

        Ok(msg)
    }

    pub async fn recv_request(&mut self) -> Result<Option<(bool, u16)>, ServerError> {
        let msg = self.recv_message().await?;
        match msg {
            Message::Request(blocking, id) => {
                self.0 = ServerState::Busy;
                Ok(Some((blocking, id)))
            }
            Message::Done => {
                self.0 = ServerState::Done;
                Ok(None)
            }
            _ => Err(ServerError::InvalidInbound),
        }
    }

    pub async fn send_response(&mut self, objs: Vec<TraceObject>) -> Result<(), ServerError> {
        let msg = Message::Response(objs);
        self.send_message(&msg).await?;
        self.0 = ServerState::Idle;
        debug!("sent trace objects response");

        Ok(())
    }
}
