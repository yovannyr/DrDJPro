//! Verify the provider's signed identity. Public keys come only from its pinned JWKS.
use super::{opaque, Digest, Engine, Sha256, INVALID, ISSUER, URL_SAFE_NO_PAD};
use ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    alg: Option<String>,
    #[serde(rename = "use")]
    purpose: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: String,
    crit: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Several(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: Audience,
    azp: Option<String>,
    exp: u64,
    nbf: Option<u64>,
    iat: u64,
    sub: String,
    nonce: Option<String>,
    at_hash: Option<String>,
}

pub(super) struct Expected<'a> {
    pub client: &'a str,
    pub nonce: &'a str,
    pub refresh: bool,
    pub access_token: &'a str,
    pub now: u64,
}

pub(super) fn subject(jwt: &str, keys: &Jwks, expected: Expected<'_>) -> Result<String, String> {
    let (signed, signature) = jwt.rsplit_once('.').ok_or(INVALID)?;
    let (header, claims) = signed.split_once('.').ok_or(INVALID)?;
    let decode = |s: &str| URL_SAFE_NO_PAD.decode(s).map_err(|_| INVALID);
    let header: Header = serde_json::from_slice(&decode(header)?).map_err(|_| INVALID)?;
    if header.alg != "RS256" || header.kid.is_empty() || header.crit.is_some_and(|v| !v.is_empty())
    {
        return Err(INVALID.into());
    }
    let mut candidates = keys.keys.iter().filter(|j| {
        j.kid.as_ref() == Some(&header.kid)
            && j.kty == "RSA"
            && j.alg.as_deref().is_none_or(|a| a == "RS256")
            && j.purpose.as_deref().is_none_or(|p| p == "sig")
    });
    let jwk = candidates.next().ok_or(INVALID)?;
    if candidates.next().is_some() {
        return Err(INVALID.into());
    }
    RsaPublicKeyComponents {
        n: decode(jwk.n.as_deref().ok_or(INVALID)?)?,
        e: decode(jwk.e.as_deref().ok_or(INVALID)?)?,
    }
    .verify(
        &RSA_PKCS1_2048_8192_SHA256,
        signed.as_bytes(),
        &decode(signature)?,
    )
    .map_err(|_| INVALID)?;
    let claims: Claims = serde_json::from_slice(&decode(claims)?).map_err(|_| INVALID)?;
    let allowed_aud = match &claims.aud {
        Audience::One(id) => id == expected.client,
        Audience::Several(ids) => {
            ids.iter().any(|s| s == expected.client) && (ids.len() == 1 || claims.azp.is_some())
        }
    };
    if !allowed_aud
        || claims
            .azp
            .as_deref()
            .is_some_and(|id| id != expected.client)
        || claims.iss != ISSUER
        || claims.exp <= expected.now
        || claims.iat >= claims.exp
        || claims.iat > expected.now.saturating_add(60)
        || claims.nbf.is_some_and(|nbf| nbf > expected.now)
        || !opaque(&claims.sub)
        || claims.sub.len() > 255
    {
        return Err(INVALID.into());
    }
    if claims
        .nonce
        .as_deref()
        .map_or(!expected.refresh, |v| v != expected.nonce)
    {
        return Err(INVALID.into());
    }
    if let Some(hash) = claims.at_hash {
        let digest = Sha256::digest(expected.access_token.as_bytes());
        if hash != URL_SAFE_NO_PAD.encode(&digest[..16]) {
            return Err(INVALID.into());
        }
    }
    Ok(claims.sub)
}
