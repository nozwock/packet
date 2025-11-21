use rqs_lib::channel::{Message, MessageClient};

pub trait MessageExt {
    fn as_client_unchecked(&self) -> &MessageClient;
}

impl MessageExt for Message {
    fn as_client_unchecked(&self) -> &MessageClient {
        match self {
            Message::Client(message_client) => message_client,
            _ => panic!("Failed to cast Message to MessageClient"),
        }
    }
}
