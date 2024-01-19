#![no_implicit_prelude]
extern crate tarpc as some_random_other_name;

#[::tarpc::derive_serde]
#[derive(Debug, PartialEq, Eq)]
pub enum TestData {
    Black,
    White,
}

#[::tarpc::service]
pub trait ColorProtocol {
    async fn get_opposite_color(color: u8) -> u8;
}