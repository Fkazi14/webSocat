
#[derive(Debug, Clone, websocat_derive::WebsocatNode)]
#[websocat_node(
    official_name = "stdio",
    prefix="stdio",
)]
pub struct Stdio {
}

#[websocat_api::async_trait::async_trait]
impl websocat_api::Node for Stdio {
    #[tracing::instrument(level="debug", name="Stdio", err)]
    async fn run(&self, _: websocat_api::RunContext, _: Option<&mut websocat_api::IWantToServeAnotherConnection>) -> websocat_api::Result<websocat_api::Bipipe> {
        tracing::trace!("Obtaining stdin and stdout");
        let r = tokio::io::stdin();
        let w = tokio::io::stdout();
        tracing::debug!("Obtained stdin and stdout");
        Ok(websocat_api::Bipipe {
            r : websocat_api::Source::ByteStream(Box::pin(r)),
            w : websocat_api::Sink::ByteStream(Box::pin(w)),
            closing_notification: None,
        })
    }
}