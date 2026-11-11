//! Reads the user's Spotify playlists through the Spotify Web API, and finds which of their
//! songs the local library already has.
//!
//! The login uses the authorization code flow with PKCE, so the app needs only a client ID
//! and no secret. Spotify sends the browser back to a small server on the loopback address.

use std::{
    collections::HashMap,
    fmt::Write as _,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Must be listed exactly like this in the app's settings on the Spotify dashboard.
pub const REDIRECT_URI: &str = "http://127.0.0.1:43821/callback";
const REDIRECT_ADDRESS: &str = "127.0.0.1:43821";
const CALLBACK_PATH: &str = "/callback";
const SCOPES: &str = "playlist-read-private playlist-read-collaborative";
const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const API: &str = "https://api.spotify.com/v1";
/// How long the app waits for the browser to come back from the login page.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
/// An access token this close to its end counts as expired, so it cannot expire on the way.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);
/// Spotify asks clients that send too many requests to wait. Waits beyond this give up.
const MAX_RETRY_WAIT: u64 = 30;
const RETRIES: u32 = 3;
pub const NOT_SHARED: &str = "Spotify only shares the songs of playlists you own or collaborate on.";
pub const LOGIN_EXPIRED: &str = "Your Spotify login has expired. Log in again.";
const CANCELED: &str = "Spotify login was canceled.";

pub type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub tracks: usize,
    /// A temporary link, so it is not saved.
    pub image: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub name: String,
    pub artists: Vec<String>,
    pub album: String,
    pub image: Option<String>,
    pub duration_ms: u64,
}

/// What the background requests report back to the app.
pub enum Event {
    LoggedIn { session: Session, refresh_token: String },
    LoginFailed(String),
    /// Spotify replaced the refresh token, so the saved one has to change.
    TokenChanged(String),
    /// The saved login no longer works, and the user has to log in again.
    LoggedOut(String),
    Playlists(Result<(User, Vec<Playlist>), String>),
    Tracks { playlist: String, result: Result<Vec<Track>, String> },
}

enum Failure {
    LoggedOut,
    Status(u16, String),
    Other(String),
}

impl Failure {
    fn message(self) -> String {
        match self {
            Self::LoggedOut => LOGIN_EXPIRED.to_owned(),
            Self::Status(_, message) | Self::Other(message) => message,
        }
    }
}

struct Auth {
    access: String,
    refresh: String,
    expires: Instant,
}

struct Client {
    client_id: String,
    agent: ureq::Agent,
    auth: Mutex<Auth>,
    events: Sender<Event>,
    wake: Wake,
}

/// A login that works. Clones share the same tokens.
#[derive(Clone)]
pub struct Session(Arc<Client>);

/// The login page waits in the browser. Dropping this does not stop the wait, `cancel` does.
pub struct PendingLogin {
    pub url: String,
    pub cancel: Arc<AtomicBool>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into()
}

fn send(events: &Sender<Event>, wake: &Wake, event: Event) {
    // The app is gone when nobody receives, and then nothing is left to tell.
    if events.send(event).is_ok() {
        wake();
    }
}

/// Unpadded base64url, as PKCE uses.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = chunk.iter().enumerate().fold(0_u32, |bits, (index, &byte)| bits | u32::from(byte) << (16 - 8 * index));
        for index in 0..=chunk.len() {
            text.push(char::from(ALPHABET[(bits >> (18 - 6 * index) & 63) as usize]));
        }
    }
    text
}

pub fn code_challenge(verifier: &str) -> String {
    base64url(Sha256::digest(verifier.as_bytes()).as_slice())
}

fn random_text(bytes: usize) -> Result<String, String> {
    let mut random = vec![0; bytes];
    getrandom::fill(&mut random).map_err(|err| format!("Could not start the Spotify login: {err}"))?;
    Ok(base64url(&random))
}

fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = || std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok().and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], hex()) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                index += 3;
                continue;
            }
            (b'+', _) => decoded.push(b' '),
            (byte, _) => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

pub fn authorize_url(client_id: &str, challenge: &str, state: &str) -> String {
    let query = [
        ("client_id", client_id),
        ("response_type", "code"),
        ("redirect_uri", REDIRECT_URI),
        ("code_challenge_method", "S256"),
        ("code_challenge", challenge),
        ("state", state),
        ("scope", SCOPES),
    ];
    let query: Vec<String> = query.iter().map(|(key, value)| format!("{key}={}", encode(value))).collect();
    format!("{AUTHORIZE_URL}?{}", query.join("&"))
}

/// Reads the first line of the request the login page sends the browser back with. `None`
/// means some other request, such as the browser asking for an icon.
pub fn parse_callback(request_line: &str, state: &str) -> Option<Result<String, String>> {
    let target = request_line.strip_prefix("GET ")?.split(' ').next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != CALLBACK_PATH {
        return None;
    }
    let params: HashMap<&str, String> =
        query.split('&').filter_map(|pair| pair.split_once('=')).map(|(key, value)| (key, decode(value))).collect();
    if params.get("state").map(String::as_str) != Some(state) {
        return Some(Err("The Spotify login answer did not match this login. Try again.".to_owned()));
    }
    if let Some(error) = params.get("error") {
        return Some(Err(if error == "access_denied" { CANCELED.to_owned() } else { format!("Spotify login failed: {error}") }));
    }
    Some(params.get("code").filter(|code| !code.is_empty()).cloned().ok_or_else(|| "Spotify sent no login code.".to_owned()))
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn page(title: &str, text: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>amplo</title>\
         <body style=\"background:#0e0f12;color:#f2f3f5;font:16px system-ui,sans-serif;display:grid;place-items:center;height:90vh\">\
         <div style=\"text-align:center\"><h1 style=\"font-size:24px\">{}</h1><p style=\"color:#9aa1ac\">{}</p></div>",
        escape_html(title),
        escape_html(text)
    )
}

/// Answers one browser request. Returns the code once the right request came.
fn answer(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut request = Vec::new();
    let mut buffer = [0; 2048];
    while !request.windows(2).any(|pair| pair == b"\r\n") && request.len() < 16 * 1024 {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
        }
    }
    let request = String::from_utf8_lossy(&request);
    let result = parse_callback(request.lines().next().unwrap_or(""), state);
    let (status, body) = match &result {
        None => ("404 Not Found", String::new()),
        Some(Ok(_)) => ("200 OK", page("You are logged in", "Go back to amplo. You can close this tab.")),
        Some(Err(message)) => ("400 Bad Request", page("Login failed", message)),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    result
}

fn wait_for_code(listener: &TcpListener, state: &str, cancel: &AtomicBool) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|err| format!("Spotify login stopped: {err}"))?;
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELED.to_owned());
        }
        if started.elapsed() > LOGIN_TIMEOUT {
            return Err("The Spotify login took too long. Try again.".to_owned());
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(result) = answer(stream, state) {
                    return result;
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(100)),
            Err(err) => return Err(format!("Spotify login stopped: {err}")),
        }
    }
}

fn read_json(response: &mut ureq::http::Response<ureq::Body>) -> Result<Value, Failure> {
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|err| Failure::Other(format!("Could not read Spotify's answer: {err}")))?;
    serde_json::from_str(&text).map_err(|err| Failure::Other(format!("Spotify sent an answer amplo does not understand: {err}")))
}

fn network(err: ureq::Error) -> Failure {
    Failure::Other(format!("Could not reach Spotify: {err}"))
}

/// The error text of a Web API answer, or of an accounts answer.
fn error_text(status: u16, body: &Value) -> String {
    let message = body["error"]["message"]
        .as_str()
        .or_else(|| body["error_description"].as_str())
        .or_else(|| body["error"].as_str())
        .unwrap_or("no details");
    format!("Spotify answered {status}: {message}")
}

/// Sends a form to the accounts service and returns the new tokens.
fn token_request(agent: &ureq::Agent, form: &[(&str, &str)]) -> Result<(String, Option<String>, Duration), Failure> {
    let mut response = agent.post(TOKEN_URL).send_form(form.iter().copied()).map_err(network)?;
    let status = response.status().as_u16();
    let body = read_json(&mut response)?;
    if status == 400 && body["error"].as_str() == Some("invalid_grant") {
        return Err(Failure::LoggedOut);
    }
    if !(200..300).contains(&status) {
        return Err(Failure::Status(status, error_text(status, &body)));
    }
    let access = body["access_token"]
        .as_str()
        .ok_or_else(|| Failure::Other("Spotify sent no access token.".to_owned()))?
        .to_owned();
    let refresh = body["refresh_token"].as_str().map(str::to_owned);
    let lifetime = Duration::from_secs(body["expires_in"].as_u64().unwrap_or(3600));
    Ok((access, refresh, lifetime))
}

/// Opens the login server and returns the page to show in the browser. The login goes on in
/// the background, and ends with `Event::LoggedIn` or `Event::LoginFailed`.
pub fn start_login(client_id: &str, events: Sender<Event>, wake: Wake) -> Result<PendingLogin, String> {
    let client_id = client_id.trim().to_owned();
    if client_id.is_empty() {
        return Err("Paste the Client ID of your Spotify app first.".to_owned());
    }
    let listener = TcpListener::bind(REDIRECT_ADDRESS)
        .map_err(|err| format!("Could not open {REDIRECT_ADDRESS} for the Spotify login: {err}"))?;
    let verifier = random_text(64)?;
    let state = random_text(16)?;
    let url = authorize_url(&client_id, &code_challenge(&verifier), &state);
    let cancel = Arc::new(AtomicBool::new(false));
    let canceled = Arc::clone(&cancel);
    thread::spawn(move || {
        let result = wait_for_code(&listener, &state, &canceled);
        // Frees the port at once, so another login can start.
        drop(listener);
        let agent = agent();
        let result = result.and_then(|code| {
            let form = [
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", REDIRECT_URI),
                ("client_id", client_id.as_str()),
                ("code_verifier", verifier.as_str()),
            ];
            token_request(&agent, &form).map_err(Failure::message)
        });
        let event = match result {
            Ok((access, Some(refresh), lifetime)) => {
                let auth = Auth { access, refresh: refresh.clone(), expires: Instant::now() + lifetime };
                let session = Session::new(client_id, auth, agent, events.clone(), Arc::clone(&wake));
                Event::LoggedIn { session, refresh_token: refresh }
            }
            Ok((_, None, _)) => Event::LoginFailed("Spotify sent no refresh token.".to_owned()),
            Err(message) => Event::LoginFailed(message),
        };
        send(&events, &wake, event);
    });
    Ok(PendingLogin { url, cancel })
}

fn best_image(images: &Value, smallest: u64) -> Option<String> {
    let images = images.as_array()?;
    // Spotify lists the largest first. The smallest that is still large enough looks sharp.
    images
        .iter()
        .filter(|image| image["width"].as_u64().is_none_or(|width| width >= smallest))
        .last()
        .or_else(|| images.first())
        .and_then(|image| image["url"].as_str())
        .map(str::to_owned)
}

/// One page of `GET /me/playlists`, and the link to the next page.
pub fn parse_playlists(page: &Value) -> (Vec<Playlist>, Option<String>) {
    let playlists = page["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|playlist| {
            let owner = &playlist["owner"];
            Some(Playlist {
                id: playlist["id"].as_str()?.to_owned(),
                name: playlist["name"].as_str().unwrap_or_default().to_owned(),
                owner: owner["display_name"].as_str().or_else(|| owner["id"].as_str()).unwrap_or_default().to_owned(),
                // `tracks` was renamed to `items` in February 2026. Older answers still work.
                tracks: playlist["items"]["total"].as_u64().or_else(|| playlist["tracks"]["total"].as_u64()).unwrap_or(0)
                    as usize,
                image: best_image(&playlist["images"], 120),
            })
        })
        .collect();
    (playlists, page["next"].as_str().map(str::to_owned))
}

/// One page of `GET /playlists/{id}/items`. Podcast episodes and removed songs are left out.
pub fn parse_tracks(page: &Value) -> (Vec<Track>, Option<String>) {
    let tracks = page["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            // `track` was renamed to `item` in February 2026.
            let item = if row["item"].is_object() { &row["item"] } else { &row["track"] };
            if !item.is_object() || item["type"].as_str().is_some_and(|kind| kind != "track") {
                return None;
            }
            let artists = item["artists"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|artist| artist["name"].as_str())
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect();
            Some(Track {
                name: item["name"].as_str().unwrap_or_default().to_owned(),
                artists,
                album: item["album"]["name"].as_str().unwrap_or_default().to_owned(),
                image: best_image(&item["album"]["images"], 64),
                duration_ms: item["duration_ms"].as_u64().unwrap_or(0),
            })
        })
        .collect();
    (tracks, page["next"].as_str().map(str::to_owned))
}

impl Session {
    fn new(client_id: String, auth: Auth, agent: ureq::Agent, events: Sender<Event>, wake: Wake) -> Self {
        Self(Arc::new(Client { client_id, agent, auth: Mutex::new(auth), events, wake }))
    }

    /// A login saved from an earlier run. Its first request gets a new access token.
    pub fn resume(client_id: &str, refresh_token: &str, events: Sender<Event>, wake: Wake) -> Self {
        let auth = Auth { access: String::new(), refresh: refresh_token.to_owned(), expires: Instant::now() };
        Self::new(client_id.to_owned(), auth, agent(), events, wake)
    }

    fn send(&self, event: Event) {
        send(&self.0.events, &self.0.wake, event);
    }

    /// The lock stays held while a new token comes, so two requests never use the same
    /// refresh token. Spotify may accept each one only once.
    fn access_token(&self, force: bool) -> Result<String, Failure> {
        let mut auth = self.0.auth.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !force && !auth.access.is_empty() && Instant::now() + EXPIRY_MARGIN < auth.expires {
            return Ok(auth.access.clone());
        }
        let refresh = auth.refresh.clone();
        let form = [("grant_type", "refresh_token"), ("refresh_token", refresh.as_str()), ("client_id", self.0.client_id.as_str())];
        let (access, new_refresh, lifetime) = token_request(&self.0.agent, &form)?;
        auth.access = access.clone();
        auth.expires = Instant::now() + lifetime;
        if let Some(new_refresh) = new_refresh
            && new_refresh != auth.refresh
        {
            auth.refresh = new_refresh.clone();
            self.send(Event::TokenChanged(new_refresh));
        }
        Ok(access)
    }

    fn get(&self, url: &str) -> Result<Value, Failure> {
        let mut renewed = false;
        let mut retries = 0;
        loop {
            let token = self.access_token(renewed)?;
            let mut response =
                self.0.agent.get(url).header("Authorization", &format!("Bearer {token}")).call().map_err(network)?;
            let status = response.status().as_u16();
            match status {
                200..=299 => return read_json(&mut response),
                401 if !renewed => renewed = true,
                429 | 502 | 503 if retries < RETRIES => {
                    retries += 1;
                    let wait = response
                        .headers()
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok()?.trim().parse::<u64>().ok())
                        .unwrap_or(2);
                    if wait > MAX_RETRY_WAIT {
                        return Err(Failure::Status(status, "Spotify is busy. Try again in a minute.".to_owned()));
                    }
                    thread::sleep(Duration::from_secs(wait.max(1)));
                }
                _ => {
                    let body = read_json(&mut response).unwrap_or(Value::Null);
                    return Err(Failure::Status(status, error_text(status, &body)));
                }
            }
        }
    }

    fn fetch_playlists(&self) -> Result<(User, Vec<Playlist>), Failure> {
        let me = self.get(&format!("{API}/me"))?;
        let id = me["id"].as_str().unwrap_or_default().to_owned();
        let name = me["display_name"].as_str().filter(|name| !name.is_empty()).unwrap_or(&id).to_owned();
        let mut playlists = Vec::new();
        let mut next = Some(format!("{API}/me/playlists?limit=50"));
        while let Some(url) = next {
            let (page, link) = parse_playlists(&self.get(&url)?);
            playlists.extend(page);
            next = link;
        }
        Ok((User { id, name }, playlists))
    }

    fn fetch_tracks(&self, playlist: &str) -> Result<Vec<Track>, Failure> {
        let mut tracks = Vec::new();
        let mut next =
            Some(format!("{API}/playlists/{}/items?limit=50&market=from_token&additional_types=track", encode(playlist)));
        while let Some(url) = next {
            let (page, link) = parse_tracks(&self.get(&url)?);
            tracks.extend(page);
            next = link;
        }
        Ok(tracks)
    }

    /// Turns a failure into an event. A login that stopped working ends the session.
    fn report<T>(&self, result: Result<T, Failure>, event: impl FnOnce(Result<T, String>) -> Event) {
        match result {
            Err(Failure::LoggedOut) => self.send(Event::LoggedOut(LOGIN_EXPIRED.to_owned())),
            result => self.send(event(result.map_err(Failure::message))),
        }
    }

    /// Loads the user and their playlists in the background, for `Event::Playlists`.
    pub fn load_playlists(&self) {
        let session = self.clone();
        thread::spawn(move || {
            let result = session.fetch_playlists();
            session.report(result, Event::Playlists);
        });
    }

    /// Loads every song of a playlist in the background, for `Event::Tracks`.
    pub fn load_tracks(&self, playlist: &str) {
        let session = self.clone();
        let playlist = playlist.to_owned();
        thread::spawn(move || {
            let result = session.fetch_tracks(&playlist).map_err(|failure| match failure {
                Failure::Status(403, _) => Failure::Other(NOT_SHARED.to_owned()),
                failure => failure,
            });
            session.report(result, |result| Event::Tracks { playlist, result });
        });
    }
}

/// Lowercase letters and digits, with single spaces between words.
pub fn normalize(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            normalized.push(character);
        } else if character == '&' {
            normalized.push_str(" and ");
        } else {
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The title without what versions add to it: `Song (Live)`, `Song [2011 Remaster]`,
/// `Song - Radio Edit`, and `Song feat. Someone` all give `song`.
pub fn base_title(title: &str) -> String {
    let mut plain = String::with_capacity(title.len());
    let mut depth = 0_usize;
    for character in title.chars() {
        match character {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => plain.push(character),
            _ => {}
        }
    }
    let lower = plain.to_lowercase();
    let cut = [" - ", " feat. ", " feat ", " ft. ", " featuring "]
        .iter()
        .filter_map(|marker| lower.find(marker))
        .min()
        .unwrap_or(lower.len());
    let base = normalize(&lower[..cut]);
    if base.is_empty() { normalize(title) } else { base }
}

fn contains_words(text: &str, words: &str) -> bool {
    format!(" {text} ").contains(&format!(" {words} "))
}

/// Finds songs of the local library by title and artist, the way they would be written on
/// Spotify.
#[derive(Default)]
pub struct LocalIndex {
    /// Base titles, with the normalized artist and the position of each local song.
    titles: HashMap<String, Vec<(String, usize)>>,
}

impl LocalIndex {
    /// Takes the title and artist of each local song, in library order.
    pub fn new<'a>(songs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut titles: HashMap<String, Vec<(String, usize)>> = HashMap::new();
        for (index, (title, artist)) in songs.into_iter().enumerate() {
            titles.entry(base_title(title)).or_default().push((normalize(artist), index));
        }
        Self { titles }
    }

    /// The local song that is this Spotify song. A local song without an artist matches by
    /// its title alone. Otherwise one of the Spotify artists has to be among its artists.
    pub fn find(&self, track: &Track) -> Option<usize> {
        let candidates = self.titles.get(&base_title(&track.name))?;
        let artists: Vec<String> = track.artists.iter().map(|artist| normalize(artist)).filter(|a| !a.is_empty()).collect();
        candidates
            .iter()
            .find(|(local, _)| {
                local.is_empty()
                    || artists.iter().any(|artist| contains_words(local, artist) || contains_words(artist, local))
            })
            .map(|&(_, index)| index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_the_pkce_challenge_of_the_specification_example() {
        // RFC 7636, appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(code_challenge(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xFB, 0xFF]), "-_8");
    }

    #[test]
    fn verifiers_use_only_allowed_characters() {
        let verifier = random_text(64).unwrap();
        assert!((43..=128).contains(&verifier.len()));
        assert!(verifier.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
    }

    #[test]
    fn builds_the_login_link() {
        let url = authorize_url("abc", "challenge", "state");
        assert!(url.starts_with("https://accounts.spotify.com/authorize?client_id=abc&response_type=code&"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A43821%2Fcallback"));
        assert!(url.contains("scope=playlist-read-private%20playlist-read-collaborative"));
        assert!(url.contains("code_challenge_method=S256&code_challenge=challenge&state=state"));
    }

    #[test]
    fn reads_the_login_answer() {
        let line = |query: &str| format!("GET /callback?{query} HTTP/1.1");
        assert_eq!(parse_callback(&line("code=a%2Bb&state=s"), "s"), Some(Ok("a+b".to_owned())));
        assert_eq!(parse_callback(&line("error=access_denied&state=s"), "s"), Some(Err(CANCELED.to_owned())));
        assert!(matches!(parse_callback(&line("code=a&state=other"), "s"), Some(Err(_))));
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1", "s"), None);
        assert_eq!(parse_callback("POST /callback HTTP/1.1", "s"), None);
    }

    #[test]
    fn the_login_server_answers_the_browser() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let browser = thread::spawn(move || {
            let mut icon = TcpStream::connect(address).unwrap();
            icon.write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n").unwrap();
            let mut answer = String::new();
            icon.read_to_string(&mut answer).unwrap();
            assert!(answer.starts_with("HTTP/1.1 404"));
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(b"GET /callback?code=c0de&state=s HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).unwrap();
            answer
        });
        let code = wait_for_code(&listener, "s", &AtomicBool::new(false));
        assert_eq!(code, Ok("c0de".to_owned()));
        let answer = browser.join().unwrap();
        assert!(answer.starts_with("HTTP/1.1 200 OK"));
        assert!(answer.contains("You are logged in"));
    }

    #[test]
    fn a_canceled_login_stops_waiting() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        assert_eq!(wait_for_code(&listener, "s", &AtomicBool::new(true)), Err(CANCELED.to_owned()));
    }

    #[test]
    fn reads_playlists_in_the_new_and_the_old_format() {
        let page: Value = serde_json::from_str(
            r#"{"items":[
                {"id":"p1","name":"Road","owner":{"id":"me","display_name":"Me"},"items":{"total":12},
                 "images":[{"url":"big","width":640},{"url":"mid","width":300},{"url":"small","width":60}]},
                {"id":"p2","name":"Old","owner":{"id":"friend","display_name":null},"tracks":{"total":3},"images":[{"url":"one","width":null}]},
                {"id":"p3","name":"Bare","owner":{},"images":null},
                {"name":"No id"}
            ],"next":"https://api.spotify.com/v1/me/playlists?offset=50&limit=50"}"#,
        )
        .unwrap();
        let (playlists, next) = parse_playlists(&page);
        assert_eq!(next.as_deref(), Some("https://api.spotify.com/v1/me/playlists?offset=50&limit=50"));
        assert_eq!(playlists.len(), 3);
        assert_eq!(
            playlists[0],
            Playlist { id: "p1".into(), name: "Road".into(), owner: "Me".into(), tracks: 12, image: Some("mid".into()) }
        );
        assert_eq!((playlists[1].owner.as_str(), playlists[1].tracks, playlists[1].image.as_deref()), ("friend", 3, Some("one")));
        assert_eq!((playlists[2].tracks, playlists[2].image.as_deref()), (0, None));
    }

    #[test]
    fn reads_playlist_songs_and_skips_the_rest() {
        let page: Value = serde_json::from_str(
            r#"{"items":[
                {"item":{"type":"track","name":"Song","duration_ms":200000,"artists":[{"name":"A"},{"name":"B"}],
                         "album":{"name":"Album","images":[{"url":"big","width":640},{"url":"small","width":64}]}},"is_local":false},
                {"track":{"type":"track","name":"Older","artists":[{"name":"C"}],"album":{"name":"","images":[]}}},
                {"item":{"type":"episode","name":"Podcast"}},
                {"item":null,"track":true}
            ],"next":null}"#,
        )
        .unwrap();
        let (tracks, next) = parse_tracks(&page);
        assert_eq!(next, None);
        assert_eq!(tracks.len(), 2);
        assert_eq!(
            tracks[0],
            Track {
                name: "Song".into(),
                artists: vec!["A".into(), "B".into()],
                album: "Album".into(),
                image: Some("small".into()),
                duration_ms: 200_000,
            }
        );
        assert_eq!((tracks[1].name.as_str(), tracks[1].image.as_deref()), ("Older", None));
    }

    #[test]
    fn normalizes_titles_for_matching() {
        assert_eq!(normalize("  Rock & Roll!  "), "rock and roll");
        assert_eq!(normalize("Beyoncé — Halo"), "beyoncé halo");
        assert_eq!(base_title("Song (Live) [2011 Remaster]"), "song");
        assert_eq!(base_title("Song - Radio Edit"), "song");
        assert_eq!(base_title("Song feat. Someone"), "song");
        assert_eq!(base_title("(Intro)"), "intro");
        assert_eq!(base_title("夜に駆ける"), "夜に駆ける");
    }

    fn track(name: &str, artists: &[&str]) -> Track {
        Track {
            name: name.into(),
            artists: artists.iter().map(|&artist| artist.to_owned()).collect(),
            album: String::new(),
            image: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn finds_local_songs_by_title_and_artist() {
        let index = LocalIndex::new([
            ("Blinding Lights", "The Weeknd"),
            ("Levitating (feat. DaBaby)", "Dua Lipa, DaBaby"),
            ("Untitled", ""),
            ("Hello", "Adele"),
        ]);
        assert_eq!(index.find(&track("Blinding Lights", &["The Weeknd"])), Some(0));
        assert_eq!(index.find(&track("Levitating", &["Dua Lipa"])), Some(1));
        assert_eq!(index.find(&track("Untitled - 2020 Remaster", &["Anyone"])), Some(2));
        assert_eq!(index.find(&track("Hello", &["Lionel Richie"])), None);
        assert_eq!(index.find(&track("Someone Like You", &["Adele"])), None);
    }
}
