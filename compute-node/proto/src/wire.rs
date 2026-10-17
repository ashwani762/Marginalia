//! Length-prefixed messages on a QUIC stream.

use anyhow::{anyhow, bail, Result};
use serde::{de::DeserializeOwned, Serialize};

const MAX_FRAME: usize = 64 * 1024 * 1024;

pub async fn write_frame<T: Serialize>(send: &mut quinn::SendStream, msg: &T) -> Result<()> {
    let body = postcard::to_stdvec(msg)?;
    if body.len() > MAX_FRAME {
        bail!("message too large");
    }
    send.write_all(&(body.len() as u32).to_le_bytes()).await?;
    send.write_all(&body).await?;
    Ok(())
}

/// None when the other side finished the stream cleanly before a new message.
pub async fn read_frame<T: DeserializeOwned>(recv: &mut quinn::RecvStream) -> Result<Option<T>> {
    read_frame_max(recv, MAX_FRAME).await
}

/// Like [`read_frame`] with a smaller ceiling, for peers that have not proven who they are.
pub async fn read_frame_max<T: DeserializeOwned>(recv: &mut quinn::RecvStream, max: usize) -> Result<Option<T>> {
    let mut len = [0u8; 4];
    match recv.read_exact(&mut len).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(anyhow!(e)),
    }
    let n = u32::from_le_bytes(len) as usize;
    if n > max.min(MAX_FRAME) {
        bail!("message too large");
    }
    let mut body = vec![0u8; n];
    recv.read_exact(&mut body).await.map_err(|e| anyhow!(e))?;
    Ok(Some(postcard::from_bytes(&body)?))
}
