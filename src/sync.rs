//! Sync with the amplo app on a phone, over the home Wi-Fi.
//!
//! The phone plans the sync and sends the requests. This computer answers them on a TCP port:
//! it lists its songs and playlists, sends and receives song and lyrics files, and adds songs
//! to playlists. It never deletes or replaces a file. A phone finds the computer with a UDP
//! broadcast to the same port, and pairs once with a code that the computer shows.
//!
//! Each request is one line of JSON, and some have file bytes after it. Each answer is one
//! line of JSON, with file bytes after it when it says `size`.

use std::{
    collections::BTreeSet,
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, Ipv4Addr, TcpListener, TcpStream, UdpSocket},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::Sender,
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::{audio::is_audio, library};

/// The TCP port for requests, and the UDP port where phones look for the computer.
pub const PORT: u16 = 47821;
/// What a phone broadcasts to find computers that run amplo.
pub const DISCOVERY_QUESTION: &[u8] = b"amplo-sync?";
/// The version of the requests and answers. A phone with another version is told to update.
pub const VERSION: u64 = 1;
/// Requests are short. Only a playlist request with many songs comes close to this.
const MAX_LINE: u64 = 4 * 1024 * 1024;
const MAX_FILE: u64 = 8 * 1024 * 1024 * 1024;
/// After this many wrong codes the computer shows a new one, so a code cannot be guessed.
const PAIR_ATTEMPTS: u32 = 5;
/// A phone that stops talking for this long is let go.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// What the app shows or saves after a request.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncEvent {
    /// A phone paired. The app saves the key, so the phone stays paired after a restart.
    Paired(String),
    /// A short message for the title bar.
    Activity(String),
}

struct Shared {
    root: Mutex<Option<PathBuf>>,
    keys: Mutex<BTreeSet<String>>,
    code: Mutex<String>,
    wrong_codes: AtomicU32,
    stop: AtomicBool,
    name: String,
    events: Sender<SyncEvent>,
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn send(&self, event: SyncEvent) {
        if self.events.send(event).is_ok() {
            (self.wake)();
        }
    }
}

/// Answers phones until dropped.
pub struct SyncServer {
    shared: Arc<Shared>,
    port: u16,
}

impl SyncServer {
    /// Listens on `bind`. Port 0 picks a free port, as the tests do.
    pub fn start(
        bind: IpAddr,
        port: u16,
        root: Option<PathBuf>,
        keys: BTreeSet<String>,
        events: Sender<SyncEvent>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((bind, port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let discovery = UdpSocket::bind((bind, port))?;
        discovery.set_read_timeout(Some(Duration::from_millis(300)))?;
        let shared = Arc::new(Shared {
            root: Mutex::new(root),
            keys: Mutex::new(keys),
            code: Mutex::new(new_code()),
            wrong_codes: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Computer".to_owned()),
            events,
            wake: Box::new(wake),
        });

        let accepting = Arc::clone(&shared);
        thread::spawn(move || accept(&listener, &accepting));
        let answering = Arc::clone(&shared);
        thread::spawn(move || answer_discovery(&discovery, port, &answering));
        Ok(Self { shared, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The code a phone types to pair, such as `"042917"`.
    pub fn code(&self) -> String {
        self.shared.code.lock().unwrap().clone()
    }

    /// The music folder that phones sync with. `None` refuses every sync request.
    pub fn set_root(&self, root: Option<PathBuf>) {
        *self.shared.root.lock().unwrap() = root;
    }

    /// Unpairs every phone. They need the code again.
    pub fn forget_phones(&self) {
        self.shared.keys.lock().unwrap().clear();
        *self.shared.code.lock().unwrap() = new_code();
    }
}

impl Drop for SyncServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// The address of this computer on the home network, for a phone that cannot find it.
pub fn local_address() -> Option<IpAddr> {
    // Connecting a UDP socket sends nothing. It only picks the network that leads outside.
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 168, 0, 1), 9)).ok()?;
    Some(socket.local_addr().ok()?.ip()).filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
}

fn new_code() -> String {
    let mut bytes = [0_u8; 4];
    getrandom::fill(&mut bytes).expect("Windows gives random bytes");
    format!("{:06}", u32::from_le_bytes(bytes) % 1_000_000)
}

fn new_key() -> String {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes).expect("Windows gives random bytes");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn accept(listener: &TcpListener, shared: &Arc<Shared>) {
    while !shared.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let shared = Arc::clone(shared);
                thread::spawn(move || {
                    let _ = serve(stream, &shared);
                });
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(100)),
            Err(_) => thread::sleep(Duration::from_millis(500)),
        }
    }
}

fn answer_discovery(socket: &UdpSocket, port: u16, shared: &Shared) {
    let mut buffer = [0_u8; 64];
    while !shared.stop.load(Ordering::Relaxed) {
        let Ok((length, from)) = socket.recv_from(&mut buffer) else { continue };
        if &buffer[..length] == DISCOVERY_QUESTION {
            let answer = json!({ "app": "amplo", "version": VERSION, "name": shared.name, "port": port });
            let _ = socket.send_to(answer.to_string().as_bytes(), from);
        }
    }
}

/// An answer, and the file to send after it.
struct Answer {
    head: Value,
    file: Option<(fs::File, u64)>,
}

impl Answer {
    fn ok(head: Value) -> Self {
        Self { head, file: None }
    }
}

fn failure(message: &str) -> Value {
    json!({ "ok": false, "error": message })
}

fn serve(stream: TcpStream, shared: &Shared) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(IDLE_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::with_capacity(256 * 1024, stream);
    let mut line = Vec::new();
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        line.clear();
        if (&mut reader).take(MAX_LINE).read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if line.last() != Some(&b'\n') {
            // A line this long is not a request. The rest of it cannot be skipped.
            writeln!(writer, "{}", failure("The request is too long."))?;
            return Ok(());
        }
        let request: Value = match serde_json::from_slice(&line) {
            Ok(request) => request,
            Err(_) => {
                writeln!(writer, "{}", failure("The request is not JSON."))?;
                return Ok(());
            }
        };
        // A body must be read even when the request fails, or the next request starts in it.
        let body = request["size"].as_u64().unwrap_or(0);
        let answer = handle(&request, &mut reader, body, shared);
        writeln!(writer, "{}", answer.head)?;
        if let Some((mut file, size)) = answer.file {
            io::copy(&mut (&mut file).take(size), &mut writer)?;
        }
        writer.flush()?;
    }
}

fn handle(request: &Value, body: &mut impl Read, size: u64, shared: &Shared) -> Answer {
    let op = request["op"].as_str().unwrap_or("");
    let paired = request["key"].as_str().is_some_and(|key| shared.keys.lock().unwrap().contains(key));
    let wants_body = op == "put";
    if !wants_body && size > 0 {
        // Nothing else has a body. It is skipped so the next request reads right.
        let _ = io::copy(&mut body.take(size), &mut io::sink());
    }
    match op {
        "hello" => {
            return Answer::ok(json!({ "ok": true, "app": "amplo", "version": VERSION, "name": shared.name, "paired": paired }));
        }
        "pair" => return Answer::ok(pair(request, shared)),
        _ => {}
    }
    let refuse = |message: &str, body: &mut dyn Read| {
        if wants_body {
            let _ = io::copy(&mut body.take(size), &mut io::sink());
        }
        Answer::ok(failure(message))
    };
    if !paired {
        return refuse("This phone is not paired. Type the code that amplo shows on the computer.", body);
    }
    let Some(root) = shared.root.lock().unwrap().clone() else {
        return refuse("Open a music folder in amplo on the computer first.", body);
    };
    match op {
        "library" => Answer::ok(list_library(&root)),
        "get" => match open_file(&root, request["path"].as_str().unwrap_or("")) {
            Ok((file, size)) => {
                Answer { head: json!({ "ok": true, "size": size }), file: Some((file, size)) }
            }
            Err(message) => Answer::ok(failure(&message)),
        },
        "put" => {
            let path = request["path"].as_str().unwrap_or("");
            match receive_file(&root, path, body, size) {
                Ok(()) => {
                    let name = path.rsplit('/').next().unwrap_or(path);
                    if is_audio(Path::new(name)) {
                        shared.send(SyncEvent::Activity(format!("Got “{name}” from the phone")));
                    }
                    Answer::ok(json!({ "ok": true }))
                }
                Err(message) => Answer::ok(failure(&message)),
            }
        }
        "playlists" => Answer::ok(change_playlists(&root, &request["playlists"])),
        "done" => {
            let to_computer = request["toComputer"].as_u64().unwrap_or(0);
            let to_phone = request["toPhone"].as_u64().unwrap_or(0);
            shared.send(SyncEvent::Activity(format!(
                "Synced with the phone: {to_computer} new here, {to_phone} new on the phone"
            )));
            Answer::ok(json!({ "ok": true }))
        }
        _ => Answer::ok(failure("amplo on the computer does not know this request. Update it.")),
    }
}

fn pair(request: &Value, shared: &Shared) -> Value {
    let typed: String = request["code"].as_str().unwrap_or("").chars().filter(char::is_ascii_digit).collect();
    let mut code = shared.code.lock().unwrap();
    if typed == *code {
        shared.wrong_codes.store(0, Ordering::Relaxed);
        let key = new_key();
        shared.keys.lock().unwrap().insert(key.clone());
        shared.send(SyncEvent::Paired(key.clone()));
        return json!({ "ok": true, "key": key, "name": shared.name });
    }
    if shared.wrong_codes.fetch_add(1, Ordering::Relaxed) + 1 >= PAIR_ATTEMPTS {
        shared.wrong_codes.store(0, Ordering::Relaxed);
        *code = new_code();
        (shared.wake)();
        return failure("Too many wrong codes. amplo on the computer shows a new code now.");
    }
    failure("Wrong code. Look at the code in amplo on the computer.")
}

/// Names that Windows keeps for devices, with any extension.
const DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
    "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The file in the music folder at a path such as `"Band/Album/song.flac"`. Refuses paths that
/// leave the folder, names Windows does not allow, and files that are not songs or lyrics.
fn library_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let bad = || Err(format!("“{relative}” is not a song or lyrics file in the music folder."));
    let mut path = root.to_path_buf();
    let parts: Vec<&str> = relative.split('/').collect();
    for part in &parts {
        let stem = part.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        if part.is_empty()
            || *part == "."
            || *part == ".."
            || part.ends_with(['.', ' '])
            || part.chars().any(|c| c.is_control() || r#"<>:"\|?*"#.contains(c))
            || DEVICE_NAMES.contains(&stem.as_str())
        {
            return bad();
        }
        path.push(part);
    }
    let lyrics = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("lrc"));
    if !is_audio(&path) && !lyrics {
        return bad();
    }
    Ok(path)
}

/// `"Band/song.flac"` for `root\Band\song.flac`.
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path.strip_prefix(root).ok()?.components().map(|part| part.as_os_str().to_str()).collect();
    Some(parts?.join("/")).filter(|path| !path.is_empty())
}

fn list_library(root: &Path) -> Value {
    let library = library::load([root.to_path_buf()], &BTreeSet::new());
    let relative: Vec<Option<String>> = library.tracks.iter().map(|track| relative_path(root, &track.path)).collect();
    let songs: Vec<Value> = library
        .tracks
        .iter()
        .zip(&relative)
        .filter_map(|(track, path)| {
            let size = fs::metadata(&track.path).ok()?.len();
            let lyrics = track.path.with_extension("lrc").is_file();
            Some(json!({ "path": path.as_ref()?, "size": size, "lyrics": lyrics }))
        })
        .collect();
    let playlists: Vec<Value> = library
        .playlists
        .iter()
        .filter(|playlist| playlist.path.is_some())
        .map(|playlist| {
            let songs: Vec<&String> = playlist.in_file.iter().filter_map(|&track| relative[track].as_ref()).collect();
            json!({ "name": playlist.name, "songs": songs })
        })
        .collect();
    json!({ "ok": true, "songs": songs, "playlists": playlists })
}

fn open_file(root: &Path, relative: &str) -> Result<(fs::File, u64), String> {
    let path = library_path(root, relative)?;
    let file = fs::File::open(&path).map_err(|_| format!("“{relative}” is not on the computer."))?;
    let size = file.metadata().map_err(|err| err.to_string())?.len();
    Ok((file, size))
}

/// Writes a file the phone sends. A file that is there already stays as it is.
fn receive_file(root: &Path, relative: &str, body: &mut dyn Read, size: u64) -> Result<(), String> {
    let skip = |body: &mut dyn Read| {
        let _ = io::copy(&mut body.take(size), &mut io::sink());
    };
    let path = match library_path(root, relative) {
        Ok(path) if size <= MAX_FILE => path,
        Ok(_) => {
            skip(body);
            return Err("The file is too large.".to_owned());
        }
        Err(message) => {
            skip(body);
            return Err(message);
        }
    };
    if path.exists() {
        skip(body);
        return Err(format!("“{relative}” is on the computer already."));
    }
    let failed = |err: io::Error| format!("Could not save “{relative}”: {err}");
    let folder = path.parent().unwrap_or(root);
    if let Err(err) = fs::create_dir_all(folder) {
        skip(body);
        return Err(failed(err));
    }
    // The file gets its name only when it is complete, so the library never shows half a song.
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let part = folder.join(format!(".{name}.amplo-part"));
    let mut file = match fs::File::create(&part) {
        Ok(file) => file,
        Err(err) => {
            skip(body);
            return Err(failed(err));
        }
    };
    let copied = io::copy(&mut body.take(size), &mut file);
    let finished = copied.and_then(|copied| {
        file.sync_all()?;
        drop(file);
        if copied != size {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the phone stopped sending"));
        }
        if path.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "another copy came first"));
        }
        fs::rename(&part, &path)
    });
    if let Err(err) = finished {
        let _ = fs::remove_file(&part);
        return Err(failed(err));
    }
    Ok(())
}

/// Adds songs to playlists, and makes the playlists that are missing. Songs a playlist has
/// already are left out.
fn change_playlists(root: &Path, requested: &Value) -> Value {
    let library = library::load([root.to_path_buf()], &BTreeSet::new());
    let mut results = Vec::new();
    for playlist in requested.as_array().into_iter().flatten() {
        let name = playlist["name"].as_str().unwrap_or("").trim();
        let songs: Vec<PathBuf> = playlist["songs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|song| library_path(root, song.as_str()?).ok())
            .filter(|path| path.is_file())
            .collect();
        let existing = library
            .playlists
            .iter()
            .find(|known| known.path.is_some() && known.name.to_lowercase() == name.to_lowercase());
        let result = match existing.and_then(|known| known.path.as_ref()) {
            Some(path) => library::add_to_playlist(path, &songs),
            None => library::create_playlist(root, name, &songs).map(|_| songs.len()),
        };
        results.push(match result {
            Ok(added) => json!({ "name": name, "ok": true, "added": added }),
            Err(error) => json!({ "name": name, "ok": false, "error": error }),
        });
    }
    json!({ "ok": true, "playlists": results })
}

/// Talks to a server like a phone does. For the tests.
#[cfg(test)]
struct Phone {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    key: Option<String>,
}

#[cfg(test)]
impl Phone {
    fn connect(address: std::net::SocketAddr) -> Self {
        let stream = TcpStream::connect(address).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        Self { writer: stream.try_clone().unwrap(), reader: BufReader::new(stream), key: None }
    }

    fn ask(&mut self, mut request: Value, body: &[u8]) -> (Value, Vec<u8>) {
        if let Some(key) = &self.key {
            request["key"] = json!(key);
        }
        if !body.is_empty() {
            request["size"] = json!(body.len());
        }
        writeln!(self.writer, "{request}").unwrap();
        self.writer.write_all(body).unwrap();
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        let answer: Value = serde_json::from_str(&line).unwrap();
        let mut bytes = vec![0; answer["size"].as_u64().unwrap_or(0) as usize];
        self.reader.read_exact(&mut bytes).unwrap();
        (answer, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::SocketAddr, sync::mpsc};

    fn folder(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("amplo-sync-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Band")).unwrap();
        root
    }

    #[test]
    fn library_paths_stay_in_the_music_folder() {
        let root = Path::new("G:\\Music");
        assert_eq!(library_path(root, "Band/a b.flac").unwrap(), root.join("Band").join("a b.flac"));
        assert!(library_path(root, "Band/a.lrc").is_ok());
        for bad in ["../a.flac", "Band/../../a.flac", "/a.flac", "C:/a.flac", "Band\\a.flac", "a.exe", "Band/", "con.flac", "a?.mp3", "Band./a.mp3", ""] {
            assert!(library_path(root, bad).is_err(), "{bad} was allowed");
        }
        assert_eq!(relative_path(root, &root.join("Band").join("a.flac")).as_deref(), Some("Band/a.flac"));
        assert_eq!(relative_path(root, Path::new("D:\\a.flac")), None);
    }

    #[test]
    fn a_paired_phone_reads_and_adds_songs_and_playlists() {
        let root = folder("serve");
        fs::write(root.join("Band").join("a.flac"), b"song a").unwrap();
        fs::write(root.join("Band").join("a.lrc"), b"[00:01.00]Hi\n").unwrap();
        fs::write(root.join("b.mp3"), b"song b").unwrap();
        fs::write(root.join("Mix.m3u"), "#EXTM3U\r\nb.mp3\r\nBand\\a.flac\r\nmissing.mp3\r\n").unwrap();

        let (events, received) = mpsc::channel();
        let server = SyncServer::start(Ipv4Addr::LOCALHOST.into(), 0, Some(root.clone()), BTreeSet::new(), events, || {}).unwrap();
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, server.port()));
        let mut phone = Phone::connect(address);

        let (hello, _) = phone.ask(json!({ "op": "hello" }), &[]);
        assert_eq!((hello["app"].as_str(), hello["paired"].as_bool()), (Some("amplo"), Some(false)));
        let (refused, _) = phone.ask(json!({ "op": "library" }), &[]);
        assert_eq!(refused["ok"], json!(false));
        // A refused upload still reads its bytes, so the next request works.
        let (refused, _) = phone.ask(json!({ "op": "put", "path": "c.mp3" }), b"bytes");
        assert_eq!(refused["ok"], json!(false));
        let wrong = if server.code() == "000000" { "111111" } else { "000000" };
        assert_eq!(phone.ask(json!({ "op": "pair", "code": wrong }), &[]).0["ok"], json!(false));
        let (paired, _) = phone.ask(json!({ "op": "pair", "code": server.code() }), &[]);
        let key = paired["key"].as_str().unwrap().to_owned();
        assert_eq!(received.recv().unwrap(), SyncEvent::Paired(key.clone()));
        phone.key = Some(key);

        let (library, _) = phone.ask(json!({ "op": "library" }), &[]);
        let songs = library["songs"].as_array().unwrap();
        assert!(songs.contains(&json!({ "path": "Band/a.flac", "size": 6, "lyrics": true })));
        assert!(songs.contains(&json!({ "path": "b.mp3", "size": 6, "lyrics": false })));
        assert_eq!(library["playlists"], json!([{ "name": "Mix", "songs": ["b.mp3", "Band/a.flac"] }]));

        let (got, bytes) = phone.ask(json!({ "op": "get", "path": "Band/a.lrc" }), &[]);
        assert_eq!((got["ok"].as_bool(), bytes.as_slice()), (Some(true), &b"[00:01.00]Hi\n"[..]));
        assert_eq!(phone.ask(json!({ "op": "get", "path": "../secret.mp3" }), &[]).0["ok"], json!(false));

        let (put, _) = phone.ask(json!({ "op": "put", "path": "New/c.opus" }), b"song c");
        assert_eq!(put["ok"], json!(true));
        assert_eq!(fs::read(root.join("New").join("c.opus")).unwrap(), b"song c");
        assert!(matches!(received.recv().unwrap(), SyncEvent::Activity(_)));
        let (again, _) = phone.ask(json!({ "op": "put", "path": "b.mp3" }), b"other");
        assert_eq!(again["ok"], json!(false));
        assert_eq!(fs::read(root.join("b.mp3")).unwrap(), b"song b");

        let request = json!({ "op": "playlists", "playlists": [
            { "name": "mix", "songs": ["New/c.opus", "b.mp3"] },
            { "name": "Road Trip", "songs": ["b.mp3", "New/c.opus", "../x.mp3"] },
        ]});
        let (changed, _) = phone.ask(request, &[]);
        assert_eq!(changed["playlists"][0]["added"], json!(1));
        assert_eq!(changed["playlists"][1]["added"], json!(2));
        let mix = fs::read_to_string(root.join("Mix.m3u")).unwrap();
        assert!(mix.ends_with("missing.mp3\r\nNew\\c.opus\r\n"), "{mix}");
        let road = fs::read_to_string(root.join("Road Trip.m3u")).unwrap();
        assert_eq!(road, "#EXTM3U\r\nb.mp3\r\nNew\\c.opus\r\n");

        drop(server);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn too_many_wrong_codes_give_a_new_code() {
        let (events, _received) = mpsc::channel();
        let server = SyncServer::start(Ipv4Addr::LOCALHOST.into(), 0, None, BTreeSet::new(), events, || {}).unwrap();
        let mut phone = Phone::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, server.port())));
        let first = server.code();
        let wrong = if first == "000000" { "111111" } else { "000000" };
        for _ in 0..PAIR_ATTEMPTS {
            assert_eq!(phone.ask(json!({ "op": "pair", "code": wrong }), &[]).0["ok"], json!(false));
        }
        // The old code is no longer good, unless the new one is the same by chance.
        if server.code() != first {
            assert_eq!(phone.ask(json!({ "op": "pair", "code": first }), &[]).0["ok"], json!(false));
        }
        assert_eq!(phone.ask(json!({ "op": "pair", "code": server.code() }), &[]).0["ok"], json!(true));
    }

    /// Serves a test folder to a phone emulator, which reaches this computer at 10.0.2.2. Set
    /// `AMPLO_SYNC_DIR` to the folder. Listens on this computer only, for ten minutes.
    #[test]
    #[ignore = "serves a test folder to a phone emulator for ten minutes"]
    fn serve_a_test_folder() {
        let root = PathBuf::from(std::env::var_os("AMPLO_SYNC_DIR").expect("AMPLO_SYNC_DIR"));
        let (events, received) = mpsc::channel();
        let server = SyncServer::start(Ipv4Addr::LOCALHOST.into(), PORT, Some(root), BTreeSet::new(), events, || {}).unwrap();
        eprintln!("code {}", server.code());
        let deadline = std::time::Instant::now() + Duration::from_secs(600);
        while std::time::Instant::now() < deadline {
            if let Ok(event) = received.recv_timeout(Duration::from_secs(1)) {
                eprintln!("{event:?}");
            }
        }
    }

    #[test]
    fn phones_find_the_computer_with_a_broadcast() {
        let (events, _received) = mpsc::channel();
        let server = SyncServer::start(Ipv4Addr::LOCALHOST.into(), 0, None, BTreeSet::new(), events, || {}).unwrap();
        let phone = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        phone.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        phone.send_to(DISCOVERY_QUESTION, (Ipv4Addr::LOCALHOST, server.port())).unwrap();
        let mut buffer = [0_u8; 512];
        let (length, _) = phone.recv_from(&mut buffer).unwrap();
        let answer: Value = serde_json::from_slice(&buffer[..length]).unwrap();
        assert_eq!(answer["port"], json!(server.port()));
        assert_eq!(answer["app"], json!("amplo"));
    }
}
