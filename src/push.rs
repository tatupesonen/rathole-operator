use crate::error::{Error, Result};

/// POST the rendered `server.toml` to the receiver on the VPS. The receiver
/// authenticates with a bearer token and writes the file in place so rathole's
/// watcher (Modify events only) hot-reloads it.
pub async fn push_server_config(
    url: &str,
    token: &str,
    body: String,
    insecure_skip_verify: bool,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(insecure_skip_verify)
        .build()?;

    let resp = client
        .post(url)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "text/plain")
        .body(body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        return Err(Error::PushFailed(status.as_u16()));
    }
    Ok(())
}
