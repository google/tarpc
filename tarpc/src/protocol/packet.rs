use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::marker::PhantomData;

/// Packet shared between client and server.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Packet<T> {
    /// Packet id to map response to request.
    pub rpc_id: u64,
    /// Packet payload.
    pub message: T,
}

const PACKET: &'static str = "Packet";
const RPC_ID: &'static str = "rpc_id";
const MESSAGE: &'static str = "message";

impl<T: Serialize> Serialize for Packet<T> {
    #[inline]
    fn serialize<S>(&self, serializer: &mut S) -> Result<(), S::Error>
        where S: Serializer
    {
        let mut state = try!(serializer.serialize_struct(PACKET, 2));
        try!(serializer.serialize_struct_elt(&mut state, RPC_ID, &self.rpc_id));
        try!(serializer.serialize_struct_elt(&mut state, MESSAGE, &self.message));
        serializer.serialize_struct_end(state)
    }
}

impl<T: Deserialize> Deserialize for Packet<T> {
    #[inline]
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error>
        where D: Deserializer
    {
        const FIELDS: &'static [&'static str] = &[RPC_ID, MESSAGE];
        deserializer.deserialize_struct(PACKET, FIELDS, Visitor(PhantomData))
    }
}

struct Visitor<T>(PhantomData<T>);

impl<T: Deserialize> de::Visitor for Visitor<T> {
    type Value = Packet<T>;

    #[inline]
    fn visit_seq<V>(&mut self, mut visitor: V) -> Result<Packet<T>, V::Error>
        where V: de::SeqVisitor
    {
        let packet = Packet {
            rpc_id: match try!(visitor.visit()) {
                Some(rpc_id) => rpc_id,
                None => return Err(de::Error::end_of_stream()),
            },
            message: match try!(visitor.visit()) {
                Some(message) => message,
                None => return Err(de::Error::end_of_stream()),
            },
        };
        try!(visitor.end());
        Ok(packet)
    }
}

#[cfg(test)]
extern crate env_logger;

#[test]
fn serde() {
    use bincode;
    let _ = env_logger::init();

    let packet = Packet {
        rpc_id: 1,
        message: (),
    };
    let ser = bincode::serde::serialize(&packet, bincode::SizeLimit::Infinite).unwrap();
    let de = bincode::serde::deserialize(&ser);
    assert_eq!(packet, de.unwrap());
}
