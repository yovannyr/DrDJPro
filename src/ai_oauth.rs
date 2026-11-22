//! Public-client sign-in for the OpenAI open-source token sharing program.
//! All I/O takes place on the provider worker. Credentials have no Debug impl.
mod callback;
mod identity;
#[cfg(test)]
mod tests;

use crate::ai_transport::{io, AiError, Budget};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    net::TcpListener,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::{form_urlencoded, Url};
use zeroize::{Zeroize, Zeroizing};

const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const DYNAMIC: &str = "dynamic_agent_client";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const INVALID: &str = "Invalid ChatGPT authorization response. Sign in again.";
const NETWORK: &str = "Could not reach the ChatGPT authorization service.";
const DENIED: &str = "ChatGPT authorization was declined or has expired. Sign in again.";
const MAX_BODY: u64 = 96 * 1024;

pub(crate) struct Account {
    pub client: String,
    pub subject: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Tokens {
    pub access_token: String,
    refresh_token: String,
    expires_at: u64,
    scope: String,
    // Validate any nonce returned on refresh against the original login attempt.
    nonce: String,
}

impl Drop for Tokens {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.nonce.zeroize();
    }
}

impl Tokens {
    pub fn needs_refresh(&self) -> bool {
        now().saturating_add(60) >= self.expires_at
    }
    pub fn plan_enabled(&self) -> bool {
        self.scope.split_ascii_whitespace().any(|s| s == PLAN_SCOPE)
    }
}

#[derive(Deserialize)]
struct Grant {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: String,
    expires_in: u64,
    scope: Option<String>,
}

impl Grant {
    fn tokens(mut self, prior: Option<&Tokens>, nonce: &str) -> Result<Tokens, String> {
        let refresh_token = self
            .refresh_token
            .take()
            .or_else(|| prior.map(|t| t.refresh_token.clone()))
            .unwrap_or_default();
        if !self.token_type.eq_ignore_ascii_case("bearer")
            || self.expires_in == 0
            || !opaque(&self.access_token)
            || (!refresh_token.is_empty() && !opaque(&refresh_token))
        {
            return Err(INVALID.into());
        }
        Ok(Tokens {
            access_token: std::mem::take(&mut self.access_token),
            refresh_token,
            expires_at: now().saturating_add(self.expires_in),
            scope: self
                .scope
                .take()
                .or_else(|| prior.map(|t| t.scope.clone()))
                .unwrap_or_default(),
            nonce: nonce.into(),
        })
    }
}

impl Drop for Grant {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.id_token.zeroize();
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn opaque(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32_768 && value.bytes().all(|b| b.is_ascii_graphic())
}

fn client_id(value: &str) -> bool {
    opaque(value) && value.len() <= 256 && value != DYNAMIC
}

fn fresh() -> Result<Zeroizing<String>, String> {
    let mut bytes = Zeroizing::new([0; 32]);
    getrandom::fill(&mut bytes[..]).map_err(|_| "Secure randomness unavailable")?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(&bytes[..])))
}

fn remaining(cancel: &AtomicBool, deadline: Instant) -> Result<Duration, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("ChatGPT sign-in cancelled.".into());
    }
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| "ChatGPT authorization timed out.".into())
}

// Only the fixed issuer can be used in production; tests inject their own loopback server.
struct Provider {
    base: Url,
    http: Client,
}

impl Provider {
    fn open(base: &str) -> Result<Self, String> {
        let base = Url::parse(base).map_err(|_| INVALID)?;
        let mut builder = Client::builder()
            .tls_backend_native()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3));
        if base.host_str() == Some("127.0.0.1") {
            builder = builder.no_proxy();
        }
        Ok(Self {
            base,
            http: builder.build().map_err(|_| NETWORK)?,
        })
    }
    fn endpoint(&self, path: &str) -> Result<Url, String> {
        self.base.join(path).map_err(|_| INVALID.into())
    }
    fn form(&self, endpoint: Url, fields: &[(&str, &str)]) -> RequestBuilder {
        let body = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields.iter().copied())
            .finish();
        self.http
            .post(endpoint)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
    }
    fn json<T: serde::de::DeserializeOwned>(
        &self,
        req: RequestBuilder,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<T, String> {
        let response = send(req, cancel, deadline)?;
        if !response.status().is_success() {
            return Err(if response.status().is_client_error() {
                DENIED
            } else {
                NETWORK
            }
            .into());
        }
        let mut bytes = Zeroizing::new(Vec::new());
        response
            .take(MAX_BODY + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                error
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<AiError>())
                    .copied()
                    .map_or_else(|| NETWORK.into(), oauth_error)
            })?;
        remaining(cancel, deadline)?;
        if bytes.len() as u64 > MAX_BODY {
            return Err(INVALID.into());
        }
        serde_json::from_slice(&bytes).map_err(|_| INVALID.into())
    }

    fn verify(
        &self,
        jwt: &str,
        expected: identity::Expected<'_>,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<String, String> {
        let keys = self.json(
            self.http.get(self.endpoint("/.well-known/jwks.json")?),
            cancel,
            deadline,
        )?;
        identity::subject(jwt, &keys, expected)
    }

    fn login(
        &self,
        host: &str,
        previous: Option<&Account>,
        cancel: &AtomicBool,
        nonce: &str,
        allowed: impl Fn() -> Result<(), String>,
        launch: impl FnOnce(&Url) -> Result<(), String>,
    ) -> Result<(Account, Tokens), String> {
        let deadline = Instant::now() + Duration::from_secs(180);
        remaining(cancel, deadline)?;
        if previous.is_some_and(|a| !client_id(&a.client)) || !opaque(host) || host.len() > 128 {
            return Err(INVALID.into());
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|_| "Could not open the local sign-in callback.")?;
        let addr = listener.local_addr().map_err(|_| NETWORK)?;
        let redirect = format!("http://{addr}/auth/callback");
        let verifier = fresh()?;
        let state = fresh()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = self.endpoint("/api/accounts/authorize")?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("client_id", previous.map_or(DYNAMIC, |a| a.client.as_str()))
                .append_pair("response_type", "code")
                .append_pair("ext_agent_host_id", host)
                .append_pair("scope", SCOPES)
                .append_pair("resource", RESOURCE)
                .append_pair("redirect_uri", &redirect)
                .append_pair("nonce", nonce)
                .append_pair("state", &state)
                .append_pair("code_challenge_method", "S256")
                .append_pair("code_challenge", &challenge);
            if previous.is_none() {
                query.append_pair("agent_name_hint", "Dr. DJ Pro");
            }
        }
        allowed()?;
        launch(&url)?;
        let (code, issued) = callback::receive(&listener, &state, cancel, deadline)?;
        let client = match (previous, issued) {
            (Some(a), supplied) if supplied.as_ref().is_none_or(|s| s == &a.client) => {
                a.client.clone()
            }
            (None, Some(id)) if client_id(&id) => id,
            _ => return Err(INVALID.into()),
        };
        allowed()?;
        let grant: Grant = self.json(
            self.form(
                self.endpoint("/api/accounts/oauth/token")?,
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", &client),
                    ("code", &code),
                    ("code_verifier", &verifier),
                    ("redirect_uri", &redirect),
                    ("resource", RESOURCE),
                ],
            ),
            cancel,
            deadline,
        )?;
        allowed()?;
        let subject = self.verify(
            grant.id_token.as_deref().ok_or(INVALID)?,
            identity::Expected {
                client: &client,
                nonce,
                refresh: false,
                access_token: &grant.access_token,
                now: now(),
            },
            cancel,
            deadline,
        )?;
        if previous.is_some_and(|a| a.subject != subject) {
            return Err(INVALID.into());
        }
        Ok((Account { client, subject }, grant.tokens(None, nonce)?))
    }

    fn renew(
        &self,
        account: &Account,
        tokens: &Tokens,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<Tokens, String> {
        if !client_id(&account.client) || !opaque(&tokens.refresh_token) {
            return Err(DENIED.into());
        }
        let grant: Grant = self.json(
            self.form(
                self.endpoint("/api/accounts/oauth/token")?,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", &account.client),
                    ("refresh_token", &tokens.refresh_token),
                    ("resource", RESOURCE),
                ],
            ),
            cancel,
            deadline,
        )?;
        if let Some(id) = &grant.id_token {
            let sub = self.verify(
                id,
                identity::Expected {
                    client: &account.client,
                    nonce: &tokens.nonce,
                    refresh: true,
                    access_token: &grant.access_token,
                    now: now(),
                },
                cancel,
                deadline,
            )?;
            if sub != account.subject {
                return Err(INVALID.into());
            }
        }
        grant.tokens(Some(tokens), &tokens.nonce)
    }

    fn end(&self, account: &Account, tokens: &Tokens) -> Result<(), String> {
        if !client_id(&account.client) || !opaque(&tokens.refresh_token) {
            return Err(DENIED.into());
        }
        #[derive(Deserialize)]
        struct Discovery {
            issuer: String,
            revocation_endpoint: String,
        }
        let cancel = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(6);
        let discovery: Discovery = self.json(
            self.http
                .get(self.endpoint("/.well-known/openid-configuration")?),
            &cancel,
            deadline,
        )?;
        let endpoint = Url::parse(&discovery.revocation_endpoint).map_err(|_| INVALID)?;
        if discovery.issuer != ISSUER
            || endpoint.origin() != self.base.origin()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(INVALID.into());
        }
        for attempt in 0..2 {
            let req = self.form(
                endpoint.clone(),
                &[
                    ("token", &tokens.refresh_token),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", &account.client),
                ],
            );
            match send(req, &cancel, deadline) {
                Ok(r) if r.status() == reqwest::StatusCode::OK => return Ok(()),
                Ok(r) if !r.status().is_server_error() => break,
                _ if attempt == 0 => std::thread::sleep(Duration::from_millis(150)),
                _ => break,
            }
        }
        Err("Remote revocation was not confirmed. Disconnect this app in ChatGPT Settings.".into())
    }
}

fn oauth_error(error: AiError) -> String {
    match error {
        AiError::Cancelled => "ChatGPT sign-in cancelled.",
        AiError::Timeout => "ChatGPT authorization timed out.",
        _ => NETWORK,
    }
    .into()
}

fn send(
    req: RequestBuilder,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<io::Response<'_>, String> {
    let budget = Budget {
        until: deadline.min(Instant::now() + Duration::from_secs(8)),
        cancelled: cancel,
    };
    io::send(req, budget).map_err(oauth_error)
}

pub(crate) fn sign_in(
    host: &str,
    previous: Option<&Account>,
    cancel: &AtomicBool,
    allowed: impl Fn() -> Result<(), String>,
) -> Result<(Account, Tokens), String> {
    Provider::open(ISSUER)?.login(host, previous, cancel, &fresh()?, allowed, |url| {
        webbrowser::open(url.as_str()).map_err(|_| "Could not open your system browser.".into())
    })
}

pub(crate) fn refresh(
    account: &Account,
    tokens: &Tokens,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<Tokens, String> {
    remaining(cancel, deadline)?;
    Provider::open(ISSUER)?.renew(account, tokens, cancel, deadline)
}

pub(crate) fn revoke(account: &Account, tokens: &Tokens) -> Result<(), String> {
    Provider::open(ISSUER)?.end(account, tokens)
}
