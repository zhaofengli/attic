use super::*;

use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::process::Command;

use serde::de::DeserializeOwned;

pub mod test_nar;

fn connect() -> NixStore {
    NixStore::connect().expect("Failed to connect to the Nix store")
}

/// Evaluates a Nix expression using the command-line interface.
fn cli_eval<T>(expression: &str) -> T
where
    T: DeserializeOwned,
{
    let cli = Command::new("nix-instantiate")
        .args(["--eval", "--json", "-E", expression])
        .output()
        .expect("Failed to evaluate");

    if !cli.status.success() {
        panic!("Evaluation of '{}' failed: {:?}", expression, cli.status);
    }

    let json = std::str::from_utf8(&cli.stdout).expect("Result not valid UTF-8");

    serde_json::from_str(json).expect("Failed to parse output")
}

fn assert_base_name(store: &str, path: &str, expected: &str) {
    let expected = PathBuf::from(expected);

    assert_eq!(
        expected,
        to_base_name(store.as_ref(), path.as_ref()).unwrap(),
    );
}

fn assert_base_name_err(store: &str, path: &str, err: &str) {
    let e = to_base_name(store.as_ref(), path.as_ref()).unwrap_err();

    if let AtticError::InvalidStorePath { path: _, reason } = e {
        assert!(reason.contains(err));
    } else {
        panic!("to_base_name didn't return an InvalidStorePath");
    }
}

#[test]
fn test_connect() {
    connect();
}

#[test]
fn test_store_dir() {
    let store = connect();
    let expected: PathBuf = cli_eval("builtins.storeDir");
    assert_eq!(store.store_dir(), expected);
}

#[test]
fn test_to_base_name() {
    assert_base_name(
        "/nix/store",
        "/nix/store/3iq73s1p4mh4mrflj2k1whkzsimxf0l7-firefox-91.0",
        "3iq73s1p4mh4mrflj2k1whkzsimxf0l7-firefox-91.0",
    );
    assert_base_name(
        "/gnu/store",
        "/gnu/store/3iq73s1p4mh4mrflj2k1whkzsimxf0l7-firefox-91.0/",
        "3iq73s1p4mh4mrflj2k1whkzsimxf0l7-firefox-91.0",
    );
    assert_base_name(
        "/nix/store",
        "/nix/store/3iq73s1p4mh4mrflj2k1whkzsimxf0l7-firefox-91.0/bin/firefox",
        "3iq73s1p4mh4mrflj2k1whkzsimxf0l7-firefox-91.0",
    );
    assert_base_name_err(
        "/gnu/store",
        "/nix/store/3iq73s1p4mh4mrflj2k1whkzsimxf0l7-firefox-91.0",
        "Path is not in store directory",
    );
    assert_base_name_err("/nix/store", "/nix/store", "Path is store directory itself");
    assert_base_name_err(
        "/nix/store",
        "/nix/store/",
        "Path is store directory itself",
    );
    assert_base_name_err("/nix/store", "/nix/store/tooshort", "Path is too short");
}

#[test]
fn test_base_name() {
    let bn = PathBuf::from("ia70ss13m22znbl8khrf2hq72qmh5drr-ruby-2.7.5");
    StorePath::from_base_name(bn).unwrap();

    // name has invalid UTF-8
    let osstr = OsStr::from_bytes(b"ia70ss13m22znbl8khrf2hq72qmh5drr-\xc3");
    let bn = PathBuf::from(osstr);
    StorePath::from_base_name(bn).unwrap_err();

    // hash has bad characters
    let bn = PathBuf::from("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-ruby-2.7.5");
    StorePath::from_base_name(bn).unwrap_err();

    // name has bad characters
    let bn = PathBuf::from("ia70ss13m22znbl8khrf2hq72qmh5drr-shocking!!!");
    StorePath::from_base_name(bn).unwrap_err();

    // name portion empty
    let bn = PathBuf::from("ia70ss13m22znbl8khrf2hq72qmh5drr-");
    StorePath::from_base_name(bn).unwrap_err();

    // no name portion
    let bn = PathBuf::from("ia70ss13m22znbl8khrf2hq72qmh5drr");
    StorePath::from_base_name(bn).unwrap_err();

    // too short
    let bn = PathBuf::from("ia70ss13m22znbl8khrf2hq");
    StorePath::from_base_name(bn).unwrap_err();
}

#[test]
fn test_store_path_hash() {
    // valid base-32 hash
    let h = "ia70ss13m22znbl8khrf2hq72qmh5drr".to_string();
    StorePathHash::new(h).unwrap();

    // invalid characters
    let h = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string();
    StorePathHash::new(h).unwrap_err();
    let h = "IA70SS13M22ZNBL8KHRF2HQ72QMH5DRR".to_string();
    StorePathHash::new(h).unwrap_err();

    // too short
    let h = "ia70ss13m22znbl8khrf2hq".to_string();
    StorePathHash::new(h).unwrap_err();
}

#[tokio::test]
async fn test_nar_streaming() {
    let store = NixStore::connect().expect("Failed to connect to the Nix store");

    let test_nar = test_nar::NO_DEPS;
    test_nar.import().await.expect("Could not import test NAR");

    let target = test_nar.get_target().expect("Could not create dump target");
    let writer = target.get_writer().await.expect("Could not get writer");

    let store_path = store.parse_store_path(test_nar.path()).unwrap();

    let stream = store.nar_from_path(store_path);
    stream.write_all(writer).await.unwrap();

    target
        .validate()
        .await
        .expect("Could not validate resulting dump");
}

#[tokio::test]
async fn test_compute_fs_closure() {
    use test_nar::{WITH_DEPS_A, WITH_DEPS_B, WITH_DEPS_C};

    let store = NixStore::connect().expect("Failed to connect to the Nix store");

    for nar in [WITH_DEPS_C, WITH_DEPS_B, WITH_DEPS_A] {
        nar.import().await.expect("Could not import test NAR");

        let path = store
            .parse_store_path(nar.path())
            .expect("Could not parse store path");

        let actual: HashSet<StorePath> = store
            .compute_fs_closure(path, false, false, false)
            .await
            .expect("Could not compute closure")
            .into_iter()
            .collect();

        assert_eq!(nar.closure(), actual);
    }
}

#[tokio::test]
async fn test_compute_fs_closure_multi() {
    use test_nar::{NO_DEPS, WITH_DEPS_A, WITH_DEPS_B, WITH_DEPS_C};

    let store = NixStore::connect().expect("Failed to connect to the Nix store");

    for nar in [NO_DEPS, WITH_DEPS_C, WITH_DEPS_B, WITH_DEPS_A] {
        nar.import().await.expect("Could not import test NAR");
    }

    let mut expected = NO_DEPS.closure();
    expected.extend(WITH_DEPS_A.closure());

    let paths = vec![
        store.parse_store_path(WITH_DEPS_A.path()).unwrap(),
        store.parse_store_path(NO_DEPS.path()).unwrap(),
    ];

    let actual: HashSet<StorePath> = store
        .compute_fs_closure_multi(paths, false, false, false)
        .await
        .expect("Could not compute closure")
        .into_iter()
        .collect();

    eprintln!("Closure: {:#?}", actual);

    assert_eq!(expected, actual);
}

#[tokio::test]
async fn test_query_path_info() {
    use test_nar::{WITH_DEPS_B, WITH_DEPS_C};

    let store = NixStore::connect().expect("Failed to connect to the Nix store");

    for nar in [WITH_DEPS_C, WITH_DEPS_B] {
        nar.import().await.expect("Could not import test NAR");
    }

    let nar = WITH_DEPS_B;
    let path = store.parse_store_path(nar.path()).unwrap();
    let path_info = store
        .query_path_info(path)
        .await
        .expect("Could not query path info");

    eprintln!("Path info: {:?}", path_info);

    assert_eq!(nar.nar().len() as u64, path_info.nar_size);
    assert_eq!(
        vec![PathBuf::from(
            "3k1wymic8p7h5pfcqfhh0jan8ny2a712-attic-test-with-deps-c-final"
        ),],
        path_info.references
    );
}
