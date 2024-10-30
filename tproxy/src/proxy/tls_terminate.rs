use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::{bail, Context as _, Result};
use fs_err as fs;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::{rustls, TlsAcceptor};

use crate::main_service::AppState;

pub struct TlsTerminateProxy {
    app_state: AppState,
    dotted_base_domain: String,
    acceptor: TlsAcceptor,
}

impl TlsTerminateProxy {
    pub fn new(
        app_state: &AppState,
        dotted_base_domain: &str,
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> Result<Self> {
        let cert_pem = fs::read(cert.as_ref()).context("failed to read certificate")?;
        let key_pem = fs::read(key.as_ref()).context("failed to read private key")?;
        let certs = CertificateDer::pem_slice_iter(cert_pem.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .context("failed to parse certificate")?;
        let key = PrivateKeyDer::from_pem_slice(key_pem.as_slice())
            .context("failed to parse private key")?;

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;

        let acceptor = TlsAcceptor::from(Arc::new(config));

        Ok(Self {
            app_state: app_state.clone(),
            dotted_base_domain: dotted_base_domain.to_owned(),
            acceptor,
        })
    }

    pub(crate) async fn proxy(&self, inbound: TcpStream, sni: &str, buffer: Vec<u8>) -> Result<()> {
        let (app_id, port) = extract_app_id_and_port(sni, &self.dotted_base_domain)
            .context("failed to extract app id and port")?;
        let port = port.unwrap_or(80);
        let host = self
            .app_state
            .lock()
            .get_host(&app_id)
            .context(format!("tapp {app_id} not found"))?;
        let stream = MergedStream {
            buffer,
            buffer_cursor: 0,
            inbound,
        };
        let mut tls_stream = self
            .acceptor
            .accept(stream)
            .await
            .context("failed to accept tls connection")?;
        let mut outbound = TcpStream::connect((host.ip, port))
            .await
            .context("failed to connect to app")?;
        tokio::io::copy_bidirectional(&mut tls_stream, &mut outbound)
            .await
            .context("failed to bridge inbound and outbound")?;
        Ok(())
    }
}

#[pin_project::pin_project]
struct MergedStream {
    buffer: Vec<u8>,
    buffer_cursor: usize,
    #[pin]
    inbound: TcpStream,
}

impl AsyncRead for MergedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        let mut cursor = *this.buffer_cursor;
        if cursor < this.buffer.len() {
            let n = std::cmp::min(buf.remaining(), this.buffer.len() - cursor);
            buf.put_slice(&this.buffer[cursor..cursor + n]);
            cursor += n;
            if cursor == this.buffer.len() {
                cursor = 0;
                *this.buffer = vec![];
            }
            *this.buffer_cursor = cursor;
            return Poll::Ready(Ok(()));
        }
        this.inbound.poll_read(cx, buf)
    }
}
impl AsyncWrite for MergedStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::result::Result<usize, std::io::Error>> {
        self.project().inbound.poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        self.project().inbound.poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        self.project().inbound.poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::result::Result<usize, std::io::Error>> {
        self.project().inbound.poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inbound.is_write_vectored()
    }
}

fn extract_app_id_and_port(sni: &str, dotted_base_domain: &str) -> Result<(String, Option<u16>)> {
    // format: <app_id>[-<port>].<base_domain>
    if !sni.ends_with(dotted_base_domain) {
        bail!("sni is not a subdomain of {dotted_base_domain}");
    }
    let subdomain = sni[..sni.len() - dotted_base_domain.len()].to_string();
    if subdomain.contains('.') {
        bail!("only one level of subdomain is supported");
    }
    let mut parts = subdomain.split('-');
    let app_id = parts.next().context("no app id found")?.to_owned();
    let port = parts
        .next()
        .map(|p| p.parse().context("invalid port"))
        .transpose()?;
    if parts.next().is_some() {
        bail!("invalid sni format");
    }
    Ok((app_id, port))
}
