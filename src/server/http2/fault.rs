use tokio::sync::mpsc;

#[derive(Clone)]
pub(in crate::server) struct H2ConnectionFault {
    sender: mpsc::UnboundedSender<h2::Reason>,
}

impl H2ConnectionFault {
    pub(in crate::server) fn channel() -> (Self, mpsc::UnboundedReceiver<h2::Reason>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }

    pub(in crate::server) fn protocol_error(&self) {
        let _ = self.sender.send(h2::Reason::PROTOCOL_ERROR);
    }

    pub(in crate::server) fn enhance_your_calm(&self) {
        let _ = self.sender.send(h2::Reason::ENHANCE_YOUR_CALM);
    }
}
