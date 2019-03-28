use futures::compat::*;
use futures_legacy::task::Spawn as Spawn01;

#[allow(dead_code)]
struct Compat01As03SinkExposed<S, SinkItem> {
    inner: Spawn01<S>,
    buffer: Option<SinkItem>,
    close_started: bool,
}

pub fn exposed_compat_exec<S, SinkItem, F, T>(input: &Compat01As03Sink<S, SinkItem>, f: F) -> T
where
    F: FnOnce(&S) -> T,
{
    let exposed = unsafe { std::mem::transmute::<_, &Compat01As03SinkExposed<S, SinkItem>>(input) };
    f(exposed.inner.get_ref())
}
