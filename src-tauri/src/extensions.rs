//! Extension package storage. Provider execution is deliberately a separate layer.
use reqwest::blocking::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};
use tauri::{Emitter, Manager, State, WebviewWindow};
use zip::ZipArchive;

const RELEASES: &str =
    "https://api.github.com/repos/Boof2015/recast-extensions/releases?per_page=100";
const ASSETS: &str = "https://github.com/Boof2015/recast-extensions/releases/download/";
const MAX_ARCHIVE: u64 = 1024 * 1024 * 1024;
const MAX_INSTALLED: u64 = 4 * 1024 * 1024 * 1024;
const MAX_MANIFEST: u64 = 8 * 1024 * 1024;
const MAX_FILES: usize = 30_000;
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub publisher: String,
    pub platform: String,
    pub minimum_app_version: String,
    pub installed_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    path: String,
    bytes: u64,
    sha256: String,
    executable: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    package: Summary,
    files: Vec<Entry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Offer {
    pub package: Summary,
    pub sha256: String,
    pub download_bytes: u64,
    pub url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseIndex {
    schema_version: u32,
    packages: Vec<Offer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Installed {
    pub key: String,
    pub package: Summary,
    pub source: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    package: Summary,
    sha256: String,
    download_bytes: u64,
}

#[derive(Default)]
struct OperationState {
    owner: Option<String>,
    readers: BTreeMap<String, Vec<Weak<AtomicBool>>>,
    closing: HashSet<String>,
    exiting: bool,
}
#[derive(Default)]
struct Operation {
    state: Mutex<OperationState>,
    cancelled: AtomicBool,
}

#[derive(Clone)]
pub struct ExtensionManager {
    root: PathBuf,
    operation: Arc<Operation>,
    leases: Arc<Mutex<BTreeMap<String, Weak<()>>>>,
}

#[derive(Debug)]
pub struct Lease {
    _pin: Arc<()>,
}

impl ExtensionManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            operation: Arc::default(),
            leases: Arc::default(),
        }
    }
    pub fn busy(&self, owner: Option<&str>) -> bool {
        let state = self.operation.state.lock().unwrap();
        state
            .owner
            .as_ref()
            .is_some_and(|active| owner.is_none_or(|label| label == active))
            || state.readers.iter().any(|(label, readers)| {
                owner.is_none_or(|owner| owner == label)
                    && readers.iter().any(|reader| reader.strong_count() > 0)
            })
    }
    pub fn cancel(&self, owner: Option<&str>) -> bool {
        self.cancel_or_close(owner, false)
    }
    pub fn close(&self, owner: Option<&str>) -> bool {
        self.cancel_or_close(owner, true)
    }
    fn cancel_or_close(&self, owner: Option<&str>, close: bool) -> bool {
        let mut state = self.operation.state.lock().unwrap();
        if close {
            if let Some(owner) = owner {
                state.closing.insert(owner.into());
            } else {
                state.exiting = true;
            }
        }
        let mut busy = false;
        for (label, readers) in &state.readers {
            if owner.is_none_or(|owner| owner == label) {
                for cancel in readers.iter().filter_map(Weak::upgrade) {
                    cancel.store(true, Ordering::Relaxed);
                    busy = true;
                }
            }
        }
        if state
            .owner
            .as_ref()
            .is_some_and(|label| owner.is_none_or(|wanted| wanted == label))
        {
            self.operation.cancelled.store(true, Ordering::Relaxed);
            busy = true;
        }
        busy
    }
    pub(crate) fn begin_read(&self, owner: &str) -> Result<ReadGuard> {
        let mut state = self.operation.state.lock().unwrap();
        if state.exiting || state.closing.contains(owner) {
            return Err("This window is closing.".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let readers = state.readers.entry(owner.into()).or_default();
        readers.retain(|reader| reader.strong_count() > 0);
        readers.push(Arc::downgrade(&cancelled));
        Ok(ReadGuard(cancelled))
    }
    pub(crate) fn begin(&self, owner: &str) -> Result<Guard> {
        let mut state = self.operation.state.lock().unwrap();
        if state.exiting || state.closing.contains(owner) {
            return Err("This window is closing.".into());
        }
        if state.owner.is_some() {
            return Err("Another extension operation is in progress.".into());
        }
        self.operation.cancelled.store(false, Ordering::Relaxed);
        state.owner = Some(owner.into());
        Ok(Guard(self.operation.clone()))
    }

    fn remove_unused(&self, key: &str) -> Result<()> {
        let leases = self.leases.lock().unwrap();
        if leases.get(key).and_then(Weak::upgrade).is_some() {
            return Err(
                "This extension is in use by a batch. Clear that batch before removing it.".into(),
            );
        }
        remove(&self.root, key)
    }

    pub fn while_open<T>(&self, owner: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
        let state = self.operation.state.lock().unwrap();
        if state.exiting || state.closing.contains(owner) {
            return Err("This window is closing.".into());
        }
        action()
    }

    pub fn providers(
        &self,
        cancelled: &AtomicBool,
    ) -> Result<Vec<Arc<crate::providers::Provider>>> {
        let mut leases = self.leases.lock().unwrap();
        let mut providers = Vec::new();
        let mut seen = HashSet::new();
        let mut installed = list(&self.root)?;
        installed.sort_by(|a, b| {
            a.package
                .id
                .cmp(&b.package.id)
                .then_with(|| {
                    Version::parse(&b.package.version)
                        .unwrap()
                        .cmp(&Version::parse(&a.package.version).unwrap())
                })
                .then_with(|| a.key.cmp(&b.key))
        });
        for installed in installed {
            if seen.contains(&installed.package.id) {
                continue;
            }
            let directory = self.root.join(&installed.key);
            let root = directory.join("payload");
            let bytes =
                crate::providers::read_bounded(&directory.join("manifest.json"), MAX_MANIFEST)?;
            let manifest: Manifest = serde_json::from_slice(&bytes).map_err(io)?;
            validate_manifest(&manifest)?;
            if manifest.package != installed.package {
                return Err("Extension receipt and manifest differ.".into());
            }
            if !manifest
                .files
                .iter()
                .any(|entry| entry.path == "provider.json")
            {
                continue;
            }
            for entry in &manifest.files {
                let path = root.join(&entry.path);
                let metadata = fs::symlink_metadata(&path).map_err(io)?;
                if !metadata.file_type().is_file()
                    || metadata.len() != entry.bytes
                    || !path
                        .canonicalize()
                        .map_err(io)?
                        .starts_with(root.canonicalize().map_err(io)?)
                {
                    return Err(
                        "An installed extension file has changed. Reinstall the extension.".into(),
                    );
                }
                let (_, hash) = transfer(
                    File::open(path).map_err(io)?,
                    std::io::sink(),
                    entry.bytes,
                    cancelled,
                )?;
                if hash != entry.sha256 {
                    return Err("An installed extension failed its integrity check. Reinstall the extension.".into());
                }
            }
            let descriptor = provider_descriptor(&root, &manifest)?;
            seen.insert(installed.package.id.clone());
            let pin = leases
                .get(&installed.key)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(()));
            leases.insert(installed.key.clone(), Arc::downgrade(&pin));
            providers.push(Arc::new(crate::providers::Provider {
                key: installed.key.clone(),
                id: installed.package.id,
                name: installed.package.name,
                version: installed.package.version,
                root,
                descriptor,
                _lease: Lease { _pin: pin },
            }));
        }
        Ok(providers)
    }
}
pub(crate) struct Guard(Arc<Operation>);

pub(crate) struct ReadGuard(Arc<AtomicBool>);
impl ReadGuard {
    pub fn cancelled(&self) -> &AtomicBool {
        &self.0
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().owner = None;
    }
}

fn check_cancel(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err("Cancelled.".into())
    } else {
        Ok(())
    }
}
fn io(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn platform() -> String {
    format!(
        "{}-{}",
        if cfg!(target_os = "macos") {
            "macos"
        } else {
            std::env::consts::OS
        },
        if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            std::env::consts::ARCH
        }
    )
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub(crate) fn portable_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && value.split('/').all(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with(['.', ' '])
                && part
                    .bytes()
                    .all(|c| (32..127).contains(&c) && !b"\\:*?\"<>|".contains(&c))
                && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        })
}
fn validate_summary(package: &Summary) -> Result<()> {
    if package.id.is_empty()
        || package.id.len() > 80
        || !package.id.starts_with(|c: char| c.is_ascii_lowercase())
        || !package
            .id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b".-".contains(&c))
        || !portable_path(&package.id)
        || package.name.trim().is_empty()
        || package.name.len() > 100
        || package.description.len() > 500
        || package.publisher.trim().is_empty()
        || package.publisher.len() > 100
        || [&package.name, &package.description, &package.publisher]
            .iter()
            .any(|s| s.chars().any(char::is_control))
        || package.version.len() > 64
        || Version::parse(&package.version).is_err()
        || Version::parse(&package.minimum_app_version).is_err()
        || ![
            "macos-arm64",
            "macos-x86_64",
            "windows-x86_64",
            "linux-arm64",
            "linux-x86_64",
        ]
        .contains(&package.platform.as_str())
        || package.installed_bytes == 0
        || package.installed_bytes > MAX_INSTALLED
    {
        return Err("The extension has invalid package information.".into());
    }
    Ok(())
}
fn compatible(package: &Summary) -> Result<()> {
    validate_summary(package)?;
    if package.platform != platform() {
        return Err(format!(
            "This package is for {}, not {}.",
            package.platform,
            platform()
        ));
    }
    if Version::parse(&package.minimum_app_version).map_err(io)?
        > Version::parse(env!("CARGO_PKG_VERSION")).map_err(io)?
    {
        return Err(format!(
            "This extension requires Recast {} or newer.",
            package.minimum_app_version
        ));
    }
    Ok(())
}
fn validate_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.schema_version != 1 {
        return Err(
            "This extension uses an unsupported package format. Update Recast to install it."
                .into(),
        );
    }
    compatible(&manifest.package)?;
    if manifest.files.is_empty() || manifest.files.len() > MAX_FILES {
        return Err("The extension has an invalid file inventory.".into());
    }
    let mut paths = HashSet::new();
    let mut parents = HashSet::new();
    let mut total = 0u64;
    for entry in &manifest.files {
        if !portable_path(&entry.path)
            || !valid_hash(&entry.sha256)
            || !paths.insert(entry.path.to_ascii_lowercase())
        {
            return Err("The extension contains an unsafe or duplicate file path.".into());
        }
        let mut parent = entry.path.as_str();
        while let Some((base, _)) = parent.rsplit_once('/') {
            parents.insert(base.to_ascii_lowercase());
            parent = base;
        }
        total = total
            .checked_add(entry.bytes)
            .filter(|v| *v <= MAX_INSTALLED)
            .ok_or("The extension is too large.")?;
    }
    if !paths.is_disjoint(&parents) {
        return Err("The extension contains conflicting file paths.".into());
    }
    if total != manifest.package.installed_bytes {
        return Err("The extension's installed size does not match its inventory.".into());
    }
    Ok(())
}

fn transfer(
    mut input: impl Read,
    mut output: impl Write,
    limit: u64,
    cancelled: &AtomicBool,
) -> Result<(u64, String)> {
    let mut total = 0u64;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        check_cancel(cancelled)?;
        let count = input.read(&mut buffer).map_err(io)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .filter(|v| *v <= limit)
            .ok_or("The extension exceeds its expected size.")?;
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count]).map_err(io)?;
    }
    Ok((total, format!("{:x}", hash.finalize())))
}

fn read_manifest(archive: &mut ZipArchive<File>) -> Result<Manifest> {
    if archive.len() > MAX_FILES + 1 {
        return Err("The extension contains too many files.".into());
    }
    let mut entry = archive
        .by_name("manifest.json")
        .map_err(|_| "The file is not a Recast extension package.".to_string())?;
    if entry.size() > MAX_MANIFEST {
        return Err("The extension manifest is too large.".into());
    }
    let mut bytes = Vec::new();
    entry
        .by_ref()
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err("The extension manifest is too large.".into());
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| "The extension manifest could not be read.".to_string())?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}
fn snapshot(path: &Path, cancelled: &AtomicBool) -> Result<(tempfile::NamedTempFile, u64, String)> {
    let mut file = File::open(path).map_err(io)?;
    if !file.metadata().map_err(io)?.is_file() {
        return Err("Choose an extension package file.".into());
    }
    let mut copy = tempfile::NamedTempFile::new().map_err(io)?;
    let (size, hash) = transfer(&mut file, copy.as_file_mut(), MAX_ARCHIVE, cancelled)?;
    copy.as_file_mut().seek(SeekFrom::Start(0)).map_err(io)?;
    Ok((copy, size, hash))
}
fn open_archive(file: &File) -> Result<ZipArchive<File>> {
    let mut file = file.try_clone().map_err(io)?;
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    ZipArchive::new(file).map_err(|_| "The file is not a valid Recast extension package.".into())
}
fn provider_descriptor(root: &Path, manifest: &Manifest) -> Result<crate::providers::Descriptor> {
    let descriptor: crate::providers::Descriptor = serde_json::from_slice(
        &crate::providers::read_bounded(&root.join("provider.json"), 128 * 1024)?,
    )
    .map_err(io)?;
    descriptor.validate()?;
    if !manifest
        .files
        .iter()
        .any(|entry| entry.path == descriptor.executable && entry.executable)
    {
        return Err("The conversion provider executable is not declared.".into());
    }
    Ok(descriptor)
}

fn install_snapshot(
    root: &Path,
    file: &File,
    hash: &str,
    source: &str,
    expected: Option<&Summary>,
    cancelled: &AtomicBool,
) -> Result<Installed> {
    let mut archive = open_archive(file)?;
    let manifest = read_manifest(&mut archive)?;
    if expected.is_some_and(|summary| *summary != manifest.package) {
        return Err("The package does not match its published information.".into());
    }
    if archive.len() != manifest.files.len() + 1 {
        return Err("The extension archive does not match its file inventory.".into());
    }
    fs::create_dir_all(root).map_err(io)?;
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(root)
        .map_err(io)?;
    let inventory: BTreeMap<_, _> = manifest
        .files
        .iter()
        .map(|entry| (format!("payload/{}", entry.path), entry))
        .collect();
    let mut seen = HashSet::new();
    for index in 0..archive.len() {
        check_cancel(cancelled)?;
        let mut entry = archive.by_index(index).map_err(io)?;
        let name = entry.name().map_err(io)?.into_owned();
        let mode = entry.unix_mode().unwrap_or(0);
        if !seen.insert(name.clone())
            || entry.is_dir()
            || entry.is_symlink()
            || entry.encrypted()
            || (mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
        {
            return Err("The extension contains duplicate, linked or unsupported entries.".into());
        }
        if name == "manifest.json" {
            continue;
        }
        let expected = inventory
            .get(&name)
            .ok_or("The extension contains an undeclared file.")?;
        if entry.size() != expected.bytes {
            return Err("An extension file has an unexpected size.".into());
        }
        let path = staging.path().join("payload").join(&expected.path);
        fs::create_dir_all(path.parent().unwrap()).map_err(io)?;
        let mut target = File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io)?;
        let (size, digest) = transfer(&mut entry, &mut target, expected.bytes, cancelled)?;
        if size != expected.bytes || digest != expected.sha256 {
            return Err(format!("Integrity check failed for {}.", expected.path));
        }
        target.sync_all().map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(if expected.executable { 0o755 } else { 0o644 }),
            )
            .map_err(io)?;
        }
    }
    if manifest
        .files
        .iter()
        .any(|entry| entry.path == "provider.json")
    {
        provider_descriptor(&staging.path().join("payload"), &manifest)?;
    }
    let installed = Installed {
        key: hash.into(),
        package: manifest.package.clone(),
        source: source.into(),
    };
    write_json(&staging.path().join("manifest.json"), &manifest)?;
    write_json(&staging.path().join("receipt.json"), &installed)?;
    check_cancel(cancelled)?;
    let destination = root.join(hash);
    if destination.exists() {
        return Err("This exact extension package is already installed.".into());
    }
    fs::rename(staging.path(), destination).map_err(io)?;
    Ok(installed)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io)?;
    file.write_all(&serde_json::to_vec(value).map_err(io)?)
        .map_err(io)?;
    file.sync_all().map_err(io)
}

fn list(root: &Path) -> Result<Vec<Installed>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut installed = Vec::new();
    for entry in fs::read_dir(root).map_err(io)? {
        let entry = entry.map_err(io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !valid_hash(&name) || !entry.file_type().map_err(io)?.is_dir() {
            continue;
        }
        let bytes = fs::read(entry.path().join("receipt.json")).map_err(io)?;
        let receipt: Installed = serde_json::from_slice(&bytes).map_err(io)?;
        if receipt.key != name {
            return Err("An installed extension has an invalid receipt.".into());
        }
        validate_summary(&receipt.package)?;
        installed.push(receipt);
    }
    installed.sort_by(|a, b| {
        a.package.name.cmp(&b.package.name).then_with(|| {
            Version::parse(&b.package.version)
                .unwrap()
                .cmp(&Version::parse(&a.package.version).unwrap())
        })
    });
    Ok(installed)
}
fn remove(root: &Path, key: &str) -> Result<()> {
    if !valid_hash(key) {
        return Err("Invalid extension identifier.".into());
    }
    let path = root.join(key);
    if !fs::symlink_metadata(&path)
        .map_err(io)?
        .file_type()
        .is_dir()
    {
        return Err("The extension directory is invalid.".into());
    }
    fs::remove_dir_all(path).map_err(io)
}
fn client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("Recast/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(10))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(io)
}
fn fetch_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    url: &str,
    cancelled: &AtomicBool,
) -> Result<T> {
    check_cancel(cancelled)?;
    let response = client
        .get(url)
        .timeout(Duration::from_secs(25))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| {
            "Could not check extensions. Try again when GitHub is available.".to_string()
        })?;
    let mut bytes = Vec::new();
    transfer(response, &mut bytes, MAX_MANIFEST, cancelled)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| "The extension release information is invalid.".into())
}
#[derive(Deserialize)]
struct Release {
    draft: bool,
    prerelease: bool,
    tag_name: String,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}
fn catalog(cancelled: &AtomicBool) -> Result<Vec<Offer>> {
    let client = client()?;
    let releases: Vec<Release> = fetch_json(&client, RELEASES, cancelled)?;
    discover(releases, |url| fetch_json(&client, url, cancelled))
}
fn discover(
    releases: Vec<Release>,
    mut read_index: impl FnMut(&str) -> Result<ReleaseIndex>,
) -> Result<Vec<Offer>> {
    let mut latest: BTreeMap<String, Offer> = BTreeMap::new();
    for release in releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .take(100)
    {
        let Some(asset) = release
            .assets
            .iter()
            .find(|asset| asset.name == "extension-release.json")
        else {
            continue;
        };
        let prefix = format!("{ASSETS}{}/", release.tag_name);
        if !asset.browser_download_url.starts_with(&prefix) || asset.size > MAX_MANIFEST {
            continue;
        }
        let index = read_index(&asset.browser_download_url)?;
        if index.schema_version != 1 {
            continue;
        }
        for offer in index.packages {
            if compatible(&offer.package).is_err()
                || !valid_hash(&offer.sha256)
                || offer.download_bytes == 0
                || offer.download_bytes > MAX_ARCHIVE
                || !offer.url.starts_with(&prefix)
                || !release
                    .assets
                    .iter()
                    .any(|a| a.browser_download_url == offer.url && a.size == offer.download_bytes)
            {
                continue;
            }
            let replace = latest.get(&offer.package.id).is_none_or(|old| {
                Version::parse(&offer.package.version).unwrap()
                    > Version::parse(&old.package.version).unwrap()
            });
            if replace {
                latest.insert(offer.package.id.clone(), offer);
            }
        }
    }
    Ok(latest.into_values().collect())
}

async fn operation<T, F>(manager: &ExtensionManager, window: &WebviewWindow, action: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(PathBuf, &AtomicBool) -> Result<T> + Send + 'static,
{
    let guard = manager.begin(window.label())?;
    let root = manager.root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = action(root, &guard.0.cancelled);
        drop(guard);
        result
    })
    .await
    .map_err(io)?
}
#[tauri::command]
pub async fn extensions_list(manager: State<'_, ExtensionManager>) -> Result<Vec<Installed>> {
    let root = manager.root.clone();
    tauri::async_runtime::spawn_blocking(move || list(&root))
        .await
        .map_err(io)?
}
#[tauri::command]
pub async fn extensions_inspect(
    manager: State<'_, ExtensionManager>,
    window: WebviewWindow,
    path: String,
) -> Result<Review> {
    operation(&manager, &window, move |_, cancelled| {
        let (file, size, hash) = snapshot(Path::new(&path), cancelled)?;
        let manifest = read_manifest(&mut open_archive(file.as_file())?)?;
        Ok(Review {
            package: manifest.package,
            sha256: hash,
            download_bytes: size,
        })
    })
    .await
}
#[tauri::command]
pub async fn extensions_install_local(
    manager: State<'_, ExtensionManager>,
    window: WebviewWindow,
    path: String,
    sha256: String,
) -> Result<Installed> {
    let result = operation(&manager, &window, move |root, cancelled| {
        if !valid_hash(&sha256) {
            return Err("Invalid extension checksum.".into());
        }
        let (file, _, hash) = snapshot(Path::new(&path), cancelled)?;
        if sha256 != hash {
            return Err(
                "This file changed after you selected it. Choose it again before installing."
                    .into(),
            );
        }
        install_snapshot(&root, file.as_file(), &hash, "local", None, cancelled)
    })
    .await;
    if result.is_ok() {
        let _ = window.app_handle().emit("recast:extensions-changed", ());
    }
    result
}
#[tauri::command]
pub async fn extensions_catalog(
    manager: State<'_, ExtensionManager>,
    window: WebviewWindow,
) -> Result<Vec<Offer>> {
    operation(&manager, &window, |_, cancelled| catalog(cancelled)).await
}
#[tauri::command]
pub async fn extensions_install(
    manager: State<'_, ExtensionManager>,
    window: WebviewWindow,
    sha256: String,
) -> Result<Installed> {
    let result = operation(&manager, &window, move |root, cancelled| {
        let offer = catalog(cancelled)?
            .into_iter()
            .find(|offer| offer.sha256 == sha256)
            .ok_or("This package is no longer available. Check for extensions again.")?;
        let response = client()?
            .get(&offer.url)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|_| "The extension could not be downloaded.".to_string())?;
        let mut file = tempfile::NamedTempFile::new().map_err(io)?;
        let (size, hash) = transfer(
            response,
            file.as_file_mut(),
            offer.download_bytes,
            cancelled,
        )?;
        if size != offer.download_bytes || hash != offer.sha256 {
            return Err("The downloaded extension failed its integrity check.".into());
        }
        install_snapshot(
            &root,
            file.as_file(),
            &hash,
            "official",
            Some(&offer.package),
            cancelled,
        )
    })
    .await;
    if result.is_ok() {
        let _ = window.app_handle().emit("recast:extensions-changed", ());
    }
    result
}
#[tauri::command]
pub async fn extensions_remove(
    manager: State<'_, ExtensionManager>,
    window: WebviewWindow,
    key: String,
) -> Result<()> {
    let cloned = manager.inner().clone();
    let result = operation(&manager, &window, move |_, _| cloned.remove_unused(&key)).await;
    if result.is_ok() {
        let _ = window.app_handle().emit("recast:extensions-changed", ());
    }
    result
}
#[tauri::command]
pub fn extensions_cancel(manager: State<'_, ExtensionManager>, window: WebviewWindow) -> bool {
    manager.cancel(Some(window.label()))
}

pub fn manager(app: &tauri::AppHandle) -> State<'_, ExtensionManager> {
    app.state::<ExtensionManager>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn manifest() -> Manifest {
        Manifest {
            schema_version: 1,
            package: Summary {
                id: "test.extension".into(),
                name: "Test extension".into(),
                version: "1.0.0".into(),
                description: "Test only".into(),
                publisher: "Test".into(),
                platform: platform(),
                minimum_app_version: "0.1.0".into(),
                installed_bytes: 5,
            },
            files: vec![Entry {
                path: "bin/worker".into(),
                bytes: 5,
                sha256: format!("{:x}", Sha256::digest(b"hello")),
                executable: true,
            }],
        }
    }
    fn archive(
        manifest: &Manifest,
        name: &str,
        contents: &[u8],
        link: bool,
    ) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = ZipWriter::new(file.reopen().unwrap());
        zip.start_file("manifest.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&serde_json::to_vec(manifest).unwrap())
            .unwrap();
        if link {
            zip.add_symlink(name, "../../escape", SimpleFileOptions::default())
                .unwrap();
        } else {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(contents).unwrap();
        }
        zip.finish().unwrap();
        file
    }
    fn install_test(
        root: &Path,
        file: &tempfile::NamedTempFile,
        cancel: &AtomicBool,
    ) -> Result<Installed> {
        let (copy, _, hash) = snapshot(file.path(), cancel)?;
        install_snapshot(root, copy.as_file(), &hash, "local", None, cancel)
    }
    fn provider_archive(
        version: &str,
        descriptor: &crate::providers::Descriptor,
    ) -> tempfile::NamedTempFile {
        let descriptor = serde_json::to_vec(descriptor).unwrap();
        let mut manifest = manifest();
        manifest.package.version = version.into();
        manifest.package.installed_bytes += descriptor.len() as u64;
        manifest.files.push(Entry {
            path: "provider.json".into(),
            bytes: descriptor.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&descriptor)),
            executable: false,
        });
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = ZipWriter::new(file.reopen().unwrap());
        for (name, bytes) in [
            ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
            ("payload/provider.json", descriptor),
            ("payload/bin/worker", b"hello".to_vec()),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(&bytes).unwrap();
        }
        zip.finish().unwrap();
        file
    }
    #[test]
    fn provider_versions_are_pinned_and_checked_before_use() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ExtensionManager::new(dir.path().to_path_buf());
        let cancel = AtomicBool::new(false);
        let descriptor = crate::providers::test_descriptor();
        let old =
            install_test(dir.path(), &provider_archive("1.0.0", &descriptor), &cancel).unwrap();
        let pinned = manager.providers(&cancel).unwrap();
        assert_eq!(pinned[0].key, old.key);
        let new =
            install_test(dir.path(), &provider_archive("1.1.0", &descriptor), &cancel).unwrap();
        let current = manager.providers(&cancel).unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].key, new.key);
        assert_eq!(pinned[0].version, "1.0.0");
        assert!(manager
            .remove_unused(&old.key)
            .unwrap_err()
            .contains("in use"));
        drop(pinned);
        manager.remove_unused(&old.key).unwrap();
        fs::write(
            dir.path().join(&new.key).join("payload/bin/worker"),
            b"other",
        )
        .unwrap();
        assert!(manager
            .providers(&cancel)
            .unwrap_err()
            .contains("integrity"));
        drop(current);
        manager.remove_unused(&new.key).unwrap();
        assert!(manager.providers(&cancel).unwrap().is_empty());
    }
    #[test]
    fn invalid_provider_never_becomes_installed() {
        let dir = tempfile::tempdir().unwrap();
        let mut descriptor = crate::providers::test_descriptor();
        descriptor.executable = "bin/undeclared".into();
        let error = install_test(
            dir.path(),
            &provider_archive("1.0.0", &descriptor),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.contains("not declared"));
        assert!(list(dir.path()).unwrap().is_empty());
    }
    #[test]
    fn independent_inspections_cancel_on_their_own_window_close() {
        let manager = ExtensionManager::new(PathBuf::new());
        let first = manager.begin_read("first").unwrap();
        let second = manager.begin_read("second").unwrap();
        assert!(manager.close(Some("first")));
        assert!(first.cancelled().load(Ordering::Relaxed));
        assert!(!second.cancelled().load(Ordering::Relaxed));
        assert!(manager.while_open("first", || Ok(())).is_err());
        assert!(manager.begin_read("first").is_err());
        drop(first);
        assert!(!manager.busy(Some("first")));
        assert!(manager.busy(Some("second")));
        assert!(manager.close(None));
        assert!(second.cancelled().load(Ordering::Relaxed));
        drop(second);
        assert!(!manager.busy(None));
    }
    #[test]
    #[ignore = "requires RECAST_TEST_TEXT_EXTENSION pointing at the actual package"]
    fn packaged_text_provider_installs_converts_and_preserves_existing_files() {
        let archive = PathBuf::from(
            std::env::var_os("RECAST_TEST_TEXT_EXTENSION").expect("Set RECAST_TEST_TEXT_EXTENSION"),
        );
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let (copy, _, hash) = snapshot(&archive, &cancel).unwrap();
        let installed =
            install_snapshot(root.path(), copy.as_file(), &hash, "local", None, &cancel).unwrap();
        let manager = ExtensionManager::new(root.path().to_path_buf());
        let providers = manager.providers(&cancel).unwrap();
        let provider = &providers[0];
        let docs = tempfile::tempdir().unwrap();
        let source = docs.path().join("notes.md");
        fs::write(&source, "# Recast\n\nA real document.").unwrap();
        fs::write(docs.path().join("notes.pdf"), "existing PDF").unwrap();
        let (input, targets) = provider.inspect(&source, &cancel).unwrap();
        assert_eq!(input.kind, "documents");
        assert_eq!(targets.len(), 2);
        let selection = crate::providers::Selection {
            key: provider.key.clone(),
            target: "pdf".into(),
            settings: BTreeMap::from([("page-size".into(), "Letter".into())]),
        };
        let output = provider
            .convert(&source, None, &selection, &cancel, |_, _| {})
            .unwrap();
        assert!(fs::read(&output.path).unwrap().starts_with(b"%PDF-"));
        assert_ne!(PathBuf::from(&output.path), docs.path().join("notes.pdf"));
        assert_eq!(
            fs::read_to_string(docs.path().join("notes.pdf")).unwrap(),
            "existing PDF"
        );
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "# Recast\n\nA real document."
        );
        assert!(fs::read_dir(docs.path()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".recast-")));
        assert!(manager.remove_unused(&installed.key).is_err());
        // Run the same installed provider alongside a core image conversion,
        // then retry only the failed document using the frozen configuration.
        let broken = docs.path().join("broken.md");
        fs::write(&broken, "![Missing](absent.png)").unwrap();
        let image = docs.path().join("picture.png");
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        fs::copy(repo.join("tests/fixtures/rgba.png"), &image).unwrap();
        let image_backend = crate::image_backend::ImageBackend {
            root: repo.join("resources/image-backend"),
        };
        let audio_backend = crate::audio_backend::AudioBackend {
            root: repo.join("resources/audio-backend"),
        };
        let source = source
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let broken = broken
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let image = image.canonicalize().unwrap().to_string_lossy().into_owned();
        let inspected = crate::inputs::inspect_with_providers(
            Path::new(&source),
            &audio_backend,
            &cancel,
            &providers,
        )
        .unwrap();
        assert_eq!(inspected.extension_targets.len(), 2);
        let mut request = crate::jobs::ConversionRequest {
            paths: vec![source.clone(), broken.clone(), image],
            output_folder: None,
            image_options: Some(crate::image_backend::ImageOptions {
                target: "webp".into(),
                quality: 85,
                lossless: false,
                resize: 100,
                metadata: true,
                background: "#ffffff".into(),
                colors: 256,
                dither: true,
            }),
            image_frame_options: None,
            audio_options: None,
            video_options: None,
            video_audio_options: None,
            animation_options: None,
            video_frame_options: None,
            extension_options: std::collections::HashMap::from([
                (source, selection.clone()),
                (broken.clone(), selection),
            ]),
            providers: providers.clone(),
        };
        let jobs = crate::jobs::JobManager::default();
        let job = jobs.prepare("test", request.clone(), false).unwrap();
        let snapshot = Mutex::new(None);
        crate::jobs::run_job(&job, &image_backend, &audio_backend, |state| {
            *snapshot.lock().unwrap() = Some(state)
        });
        let first = snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(
            first
                .files
                .iter()
                .filter(|f| f.status == crate::jobs::FileStatus::Succeeded)
                .count(),
            2
        );
        assert_eq!(first.files[1].status, crate::jobs::FileStatus::Failed);
        let previous = first.files[0].output_path.clone().unwrap();
        let previous_bytes = fs::read(&previous).unwrap();
        fs::write(&broken, "# Repaired document").unwrap();
        request.providers.clear();
        for selection in request.extension_options.values_mut() {
            selection.key = "unavailable".into();
            selection.target = "docx".into();
        }
        let retry = jobs.prepare("test", request, true).unwrap();
        crate::jobs::run_job(&retry, &image_backend, &audio_backend, |state| {
            *snapshot.lock().unwrap() = Some(state)
        });
        let last = snapshot.into_inner().unwrap().unwrap();
        assert!(last
            .files
            .iter()
            .all(|f| f.status == crate::jobs::FileStatus::Succeeded));
        assert_eq!(
            last.files[0].output_path.as_deref(),
            Some(previous.as_str())
        );
        assert_eq!(fs::read(&previous).unwrap(), previous_bytes);
        assert!(last.files[1]
            .output_path
            .as_ref()
            .unwrap()
            .ends_with(".pdf"));
        drop(job);
        drop(retry);
        drop(jobs);
        drop(providers);
        manager.remove_unused(&installed.key).unwrap();
    }
    #[test]
    fn installs_persists_and_removes_without_running_payload() {
        let dir = tempfile::tempdir().unwrap();
        let file = archive(&manifest(), "payload/bin/worker", b"hello", false);
        let installed = install_test(dir.path(), &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            fs::read(dir.path().join(&installed.key).join("payload/bin/worker")).unwrap(),
            b"hello"
        );
        assert_eq!(list(dir.path()).unwrap()[0].key, installed.key);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.path().join(&installed.key).join("payload/bin/worker"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
        assert!(install_test(dir.path(), &file, &AtomicBool::new(false))
            .unwrap_err()
            .contains("already installed"));
        remove(dir.path(), &installed.key).unwrap();
        assert!(list(dir.path()).unwrap().is_empty());
        assert!(remove(dir.path(), "../elsewhere").is_err());
    }
    #[test]
    fn damaged_update_and_cancel_leave_previous_install_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let original = archive(&manifest(), "payload/bin/worker", b"hello", false);
        let installed = install_test(dir.path(), &original, &AtomicBool::new(false)).unwrap();
        let mut next = manifest();
        next.package.version = "2.0.0".into();
        let damaged = archive(&next, "payload/bin/worker", b"HELLO", false);
        assert!(install_test(dir.path(), &damaged, &AtomicBool::new(false))
            .unwrap_err()
            .contains("Integrity"));
        assert!(install_test(dir.path(), &original, &AtomicBool::new(true))
            .unwrap_err()
            .contains("Cancelled"));
        assert_eq!(list(dir.path()).unwrap()[0].key, installed.key);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn rejects_links_and_undeclared_files_without_leaving_staging_directories() {
        let dir = tempfile::tempdir().unwrap();
        for (name, link) in [
            ("payload/bin/worker", true),
            ("../escape", false),
            ("receipt.json", false),
        ] {
            let file = archive(&manifest(), name, b"hello", link);
            assert!(install_test(dir.path(), &file, &AtomicBool::new(false)).is_err());
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }
    #[test]
    fn rejects_platform_version_schema_sizes_and_unsafe_names() {
        let original = manifest();
        for path in [
            "../escape",
            "/absolute",
            "C:/absolute",
            "a\\b",
            "a/../b",
            "a//b",
            "NUL.txt",
            "COM1",
            "name.",
            "name ",
            "foo:stream",
            "a\0b",
        ] {
            let mut invalid = original.clone();
            invalid.files[0].path = path.into();
            assert!(validate_manifest(&invalid).is_err(), "{path}");
        }
        let mut invalid = original.clone();
        invalid.schema_version = 2;
        assert!(validate_manifest(&invalid).is_err());
        invalid = original.clone();
        invalid.package.platform = if platform() == "macos-arm64" {
            "windows-x86_64"
        } else {
            "macos-arm64"
        }
        .into();
        assert!(validate_manifest(&invalid).is_err());
        invalid = original.clone();
        invalid.package.minimum_app_version = "99.0.0".into();
        assert!(validate_manifest(&invalid).is_err());
        invalid = original.clone();
        invalid.package.installed_bytes = MAX_INSTALLED + 1;
        assert!(validate_manifest(&invalid).is_err());
        invalid = original.clone();
        invalid.files[0].bytes = u64::MAX;
        assert!(validate_manifest(&invalid).is_err());
        invalid = original.clone();
        invalid.files.push(invalid.files[0].clone());
        invalid.files[1].path = "BIN/WORKER".into();
        invalid.package.installed_bytes = 10;
        assert!(validate_manifest(&invalid).is_err());
        invalid = original.clone();
        invalid.files.push(invalid.files[0].clone());
        invalid.files[1].path = "bin/worker/child".into();
        invalid.package.installed_bytes = 10;
        assert!(validate_manifest(&invalid).is_err());
    }
    #[test]
    fn downloaded_metadata_must_match_package_and_transfer_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let file = archive(&manifest(), "payload/bin/worker", b"hello", false);
        let mut expected = manifest().package;
        expected.id = "other.extension".into();
        let (copy, _, hash) = snapshot(file.path(), &AtomicBool::new(false)).unwrap();
        assert!(install_snapshot(
            dir.path(),
            copy.as_file(),
            &hash,
            "official",
            Some(&expected),
            &AtomicBool::new(false)
        )
        .is_err());
        let mut output = Vec::new();
        assert!(transfer(
            b"123456".as_slice(),
            &mut output,
            5,
            &AtomicBool::new(false)
        )
        .is_err());
        assert!(output.is_empty());
    }
    #[test]
    fn snapshot_is_immutable_and_checksum_detects_source_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = archive(&manifest(), "payload/bin/worker", b"hello", false);
        let (copy, _, reviewed_hash) = snapshot(file.path(), &AtomicBool::new(false)).unwrap();
        fs::write(file.path(), b"changed").unwrap();
        let (_, _, changed_hash) = snapshot(file.path(), &AtomicBool::new(false)).unwrap();
        assert_ne!(reviewed_hash, changed_hash);
        assert!(install_snapshot(
            dir.path(),
            copy.as_file(),
            &reviewed_hash,
            "local",
            None,
            &AtomicBool::new(false)
        )
        .is_ok());
    }
    #[test]
    fn cancellation_is_owned_by_the_requesting_window_and_serializes_mutations() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ExtensionManager::new(dir.path().into());
        let guard = manager.begin("main").unwrap();
        assert!(manager.begin("batch-2").is_err());
        assert!(!manager.cancel(Some("batch-2")));
        assert!(manager.busy(Some("main")));
        assert!(manager.cancel(Some("main")));
        assert!(check_cancel(&guard.0.cancelled).is_err());
        drop(guard);
        assert!(!manager.busy(None));
        let guard = manager.begin("batch-2").unwrap();
        assert!(check_cancel(&guard.0.cancelled).is_ok());
    }
    #[test]
    fn closing_window_and_quitting_block_late_install_requests() {
        let manager = ExtensionManager::new(PathBuf::new());
        let guard = manager.begin("main").unwrap();
        assert!(manager.close(Some("main")));
        drop(guard);
        assert!(manager.begin("main").is_err());
        let second = manager.begin("batch-2").unwrap();
        assert!(manager.close(None));
        drop(second);
        assert!(manager.begin("batch-3").is_err());
    }
    fn release(version: &str, draft: bool, prerelease: bool) -> (Release, ReleaseIndex) {
        let tag = format!("test.extension-v{version}");
        let prefix = format!("{ASSETS}{tag}/");
        let mut package = manifest().package;
        package.version = version.into();
        let url = format!("{prefix}test.recast-extension");
        (
            Release {
                draft,
                prerelease,
                tag_name: tag,
                assets: vec![
                    Asset {
                        name: "extension-release.json".into(),
                        browser_download_url: format!("{prefix}extension-release.json"),
                        size: 1024,
                    },
                    Asset {
                        name: "test.recast-extension".into(),
                        browser_download_url: url.clone(),
                        size: 500,
                    },
                ],
            },
            ReleaseIndex {
                schema_version: 1,
                packages: vec![Offer {
                    package,
                    sha256: "a".repeat(64),
                    download_bytes: 500,
                    url,
                }],
            },
        )
    }
    #[test]
    fn catalog_excludes_drafts_prereleases_and_picks_semver_latest() {
        let (draft, _) = release("3.0.0", true, false);
        let (prerelease, _) = release("2.0.0", false, true);
        let (older, older_index) = release("0.9.0", false, false);
        let (newer, newer_index) = release("0.10.0", false, false);
        let mut indices = BTreeMap::from([
            (older.assets[0].browser_download_url.clone(), older_index),
            (newer.assets[0].browser_download_url.clone(), newer_index),
        ]);
        let offers = discover(vec![draft, prerelease, older, newer], |url| {
            Ok(indices
                .remove(url)
                .expect("never fetch a draft or prerelease"))
        })
        .unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].package.version, "0.10.0");
    }
    #[test]
    fn catalog_rejects_external_urls_and_assets_missing_from_release() {
        let (mut invalid, _) = release("1.0.0", false, false);
        invalid.assets[0].browser_download_url = "https://example.com/index.json".into();
        assert!(discover(vec![invalid], |_| panic!(
            "never fetch arbitrary index URLs"
        ))
        .unwrap()
        .is_empty());
        let (valid, mut index) = release("1.0.0", false, false);
        index.packages[0].url = "https://example.com/package".into();
        let mut index = Some(index);
        assert!(discover(vec![valid], |_| Ok(index.take().unwrap()))
            .unwrap()
            .is_empty());
    }
    #[test]
    #[ignore = "manual live check of the public extension release endpoint"]
    fn live_extension_catalog() {
        let result = catalog(&AtomicBool::new(false)).unwrap();
        eprintln!("{} compatible published extensions", result.len());
    }
}
