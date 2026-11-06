use crate::{request::Request, response::Response};

pub trait Service<C> {
    type Error;

    fn call(&self, ctx: &C, req: Request) -> Result<Response, Self::Error>;
}
