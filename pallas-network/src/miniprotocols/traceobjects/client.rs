use std::fmt::Debug;
use thiserror::*;
use tracing::debug;

use super::protocol::*;
use crate::multiplexer;

#[derive(Error, Debug)]
pub enum ClientError {
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

pub enum ClientState {
    Idle,
    Busy,
    Done,
}

pub struct Client(ClientState, multiplexer::ChannelBuffer);

impl Client {
    pub fn new(channel: multiplexer::AgentChannel) -> Self {
        Self(ClientState::Idle, multiplexer::ChannelBuffer::new(channel))
    }

    pub fn state(&self) -> &ClientState {
        &self.0
    }

    pub fn is_done(&self) -> bool {
        matches!(self.0, ClientState::Done)
    }

    pub fn has_agency(&self) -> bool {
        match &self.0 {
            ClientState::Idle => true,
            ClientState::Busy => false,
            ClientState::Done => false,
        }
    }

    fn assert_agency_is_ours(&self) -> Result<(), ClientError> {
        if !self.has_agency() {
            Err(ClientError::AgencyIsTheirs)
        } else {
            Ok(())
        }
    }

    fn assert_agency_is_theirs(&self) -> Result<(), ClientError> {
        if self.has_agency() {
            Err(ClientError::AgencyIsOurs)
        } else {
            Ok(())
        }
    }

    fn assert_outbound_state(&self, msg: &Message) -> Result<(), ClientError> {
        match (&self.0, msg) {
            (ClientState::Idle, Message::Request(..)) => Ok(()),
            (ClientState::Idle, Message::Done) => Ok(()),
            _ => Err(ClientError::InvalidOutbound),
        }
    }

    fn assert_inbound_state(&self, msg: &Message) -> Result<(), ClientError> {
        match (&self.0, msg) {
            (ClientState::Busy, Message::Response(..)) => Ok(()),
            _ => Err(ClientError::InvalidInbound),
        }
    }

    pub async fn send_message(&mut self, msg: &Message) -> Result<(), ClientError> {
        self.assert_agency_is_ours()?;
        self.assert_outbound_state(msg)?;
        self.1
            .send_msg_chunks(msg)
            .await
            .map_err(ClientError::Plexer)?;

        Ok(())
    }

    pub async fn recv_message(&mut self) -> Result<Message, ClientError> {
        self.assert_agency_is_theirs()?;
        let msg = self.1.recv_full_msg().await.map_err(ClientError::Plexer)?;
        self.assert_inbound_state(&msg)?;

        Ok(msg)
    }

    pub async fn send_request(&mut self, blocking: bool, id: u16) -> Result<(), ClientError> {
        let msg = Message::Request(blocking, id);
        self.send_message(&msg).await?;
        self.0 = ClientState::Busy;
        debug!("sent trace request message");

        Ok(())
    }

    pub async fn recv_response(&mut self) -> Result<Vec<TraceObject>, ClientError> {
        let msg = self.recv_message().await?;
        match msg {
            Message::Response(objs) => {
                debug!(length = objs.len(), "received trace objects");
                self.0 = ClientState::Idle;
                Ok(objs)
            }
            _ => Err(ClientError::InvalidInbound),
        }
    }

    pub async fn send_done(&mut self) -> Result<(), ClientError> {
        let msg = Message::Done;
        self.send_message(&msg).await?;
        self.0 = ClientState::Done;

        Ok(())
    }
}
