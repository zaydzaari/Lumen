//! Disposable provider sign-in / browser-attestation process. The main player
//! retains no webview after the result. Credentials travel only through pipes.
use crate::providers::Kind;
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read as _, Write as _},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub kind: Kind,
    pub binding: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Reply {
    cookie: Option<String>,
}
pub struct Browser {
    child: Child,
    rx: mpsc::Receiver<Option<String>>,
    opened: Instant,
}
impl Browser {
    pub fn open(request: Request) -> Result<Self> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--provider-browser")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .context("Could not start provider browser")?;
        let input = serde_json::to_vec(&request)?;
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(&input) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow::anyhow!("Provider browser pipe is unavailable"));
        };
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::Builder::new()
            .name("lumen-provider-browser".into())
            .spawn(move || {
                let mut bytes = Vec::new();
                let answer = stdout
                    .take(65537)
                    .read_to_end(&mut bytes)
                    .ok()
                    .filter(|_| bytes.len() <= 65536)
                    .and_then(|_| serde_json::from_slice::<Reply>(&bytes).ok())
                    .and_then(|r| r.cookie);
                let _ = tx.send(answer);
            });
        if let Err(error) = reader {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        Ok(Self {
            child,
            rx,
            opened: Instant::now(),
        })
    }
    pub fn poll(&mut self) -> Option<Option<String>> {
        match self.rx.try_recv() {
            Ok(answer) => Some(answer),
            Err(mpsc::TryRecvError::Disconnected) => Some(None),
            Err(_) if self.opened.elapsed() > Duration::from_secs(600) => Some(None),
            Err(_) => None,
        }
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn run() -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin().take(4097).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 4096, "Provider browser request is too large");
    let request: Request = serde_json::from_slice(&bytes)?;
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    let deadline = Instant::now()
        + if request.binding.is_some() {
            Duration::from_secs(50)
        } else {
            Duration::from_secs(600)
        };
    let target = target(&request)?;
    let mut page = sonora_webview::Page::open(target)?;
    let cookie = loop {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
            };
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        match page.poll() {
            sonora_webview::Poll::Cookies(cookie) => break Some(cookie),
            sonora_webview::Poll::Closed => break None,
            sonora_webview::Poll::Pending if Instant::now() >= deadline => break None,
            _ => std::thread::sleep(Duration::from_millis(30)),
        }
    };
    let answer = serde_json::to_vec(&Reply { cookie })?;
    std::io::stdout().write_all(&answer)?;
    Ok(())
}
fn target(request: &Request) -> Result<sonora_webview::Target> {
    if let Some(binding) = &request.binding {
        anyhow::ensure!(
            request.kind == Kind::Youtube && !binding.is_empty() && binding.len() <= 1024,
            "Invalid provider attestation request"
        );
        let mut url = url::Url::parse("https://www.youtube.com/")?;
        url.query_pairs_mut().append_pair("binding", binding);
        return Ok(sonora_webview::Target {
            url: url.into(),
            landing: "www.youtube.com".into(),
            domain: "youtube.com".into(),
            proof: vec!["SONORA_POT".into()],
            title: String::new(),
            agent: None,
            script: Some(include_str!("../vendor/sonora/potoken.js").into()),
        });
    }
    let provider = request
        .kind
        .provider()
        .context("Choose a provider with browser sign-in")?;
    let sign_in = provider
        .web_sign_in()
        .context("This provider uses server credentials")?;
    Ok(sonora_webview::Target {
        url: sign_in.url.into(),
        landing: sign_in.landing.into(),
        domain: sign_in.domain.into(),
        proof: sign_in.proof.iter().map(|s| (*s).into()).collect(),
        title: format!("Lumen — {} sign-in", request.kind.label()),
        agent: sign_in.agent.map(str::to_owned),
        script: None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_requests_only_open_compiled_in_service_targets() {
        assert!(target(&Request {
            kind: Kind::Subsonic,
            binding: None
        })
        .is_err());
        assert!(target(&Request {
            kind: Kind::Apple,
            binding: Some("test".into())
        })
        .is_err());
        let target = target(&Request {
            kind: Kind::Youtube,
            binding: Some("a&other=x".into()),
        })
        .unwrap();
        assert_eq!(
            url::Url::parse(&target.url).unwrap().query_pairs().count(),
            1
        );
        assert_eq!(target.domain, "youtube.com");
    }
}
