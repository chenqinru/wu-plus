use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tempfile::TempDir;
use wu_plus_subversion_provider::{Cancellation, handle};

fn svn(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("svn")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("SVN 1.14");
    assert!(
        output.status.success(),
        "svn {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn request(method: &str, payload: Value) -> Value {
    let operation = payload["operation"].as_str().unwrap_or(method).to_owned();
    handle(method, payload, &Cancellation::default())
        .unwrap_or_else(|error| panic!("{method}/{operation}: {} ({})", error.message, error.kind))
}

fn root_request(method: &str, root: &Path, mut payload: Value) -> Value {
    payload["root"] = json!(root);
    request(method, payload)
}

fn entries(root: &Path) -> Vec<Value> {
    root_request("status", root, json!({}))["entries"]
        .as_array()
        .unwrap()
        .clone()
}

struct Repos {
    tmp: TempDir,
    a: PathBuf,
    b: PathBuf,
    url: String,
}

impl Repos {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let server = tmp.path().join("repository");
        assert!(
            Command::new("svnadmin")
                .arg("create")
                .arg(&server)
                .status()
                .unwrap()
                .success()
        );
        let url = format!("file://{}", server.display());
        svn(
            tmp.path(),
            &[
                "mkdir",
                &format!("{url}/trunk"),
                &format!("{url}/branches"),
                &format!("{url}/tags"),
                "-m",
                "layout",
            ],
        );
        let a = tmp.path().join("first");
        let b = tmp.path().join("second");
        request(
            "write",
            json!({"operation":"checkout","url":format!("{url}/trunk"),"destination":a}),
        );
        fs::write(a.join("text.txt"), "original\nline two\n").unwrap();
        fs::write(a.join("unchanged.txt"), "leave this file alone\n").unwrap();
        svn(&a, &["add", "text.txt", "unchanged.txt"]);
        svn(&a, &["commit", "-m", "seed"]);
        request(
            "write",
            json!({"operation":"checkout","url":format!("{url}/trunk"),"destination":b}),
        );
        Self { tmp, a, b, url }
    }
}

#[test]
fn history_is_empty_at_revision_zero() {
    let tmp = TempDir::new().unwrap();
    let repository = tmp.path().join("empty-repository");
    assert!(
        Command::new("svnadmin")
            .arg("create")
            .arg(&repository)
            .status()
            .unwrap()
            .success()
    );
    let root = tmp.path().join("empty-working-copy");
    request(
        "write",
        json!({"operation":"checkout","url":format!("file://{}", repository.display()),"destination":root}),
    );
    let history = root_request("read", &root, json!({"operation":"history","path":"."}));
    assert_eq!(history["revisions"], json!([]));
    assert!(history["next_before"].is_null());
}

#[test]
fn discovery_and_precise_commits_cover_nested_copies_and_special_names() {
    let fixture = Repos::new();
    let root = &fixture.a;
    let nested = root.join("embedded");
    request(
        "write",
        json!({"operation":"checkout","url":format!("{}/trunk",fixture.url),"destination":nested}),
    );
    fs::create_dir(root.join("新目录")).unwrap();
    fs::write(root.join("新目录/选中 @.txt"), "one\n").unwrap();
    fs::write(root.join("新目录/留在本地.txt"), "two\n").unwrap();
    fs::write(root.join("unchanged.txt"), "locally edited\n").unwrap();

    let copies = request(
        "discover",
        json!({"roots":[fixture.tmp.path(), root.join("新目录"), &fixture.b]}),
    );
    let roots = copies["copies"].as_array().unwrap();
    assert_eq!(roots.len(), 3, "{copies}");
    for copy in roots {
        assert_eq!(copy["repository_url"], fixture.url);
    }
    root_request("write", root, json!({"operation":"add","paths":["新目录"]}));
    let context = root_request(
        "read",
        root,
        json!({"operation":"commit_diff","paths":["新目录/选中 @.txt"]}),
    );
    let context = context["text"].as_str().unwrap();
    assert!(context.contains("added directory 新目录"));
    assert!(context.contains("新目录/选中 @.txt"));
    assert!(context.lines().any(|line| line == "+one"), "{context}");
    assert!(!context.contains("留在本地.txt"));
    assert!(!context.contains("locally edited"));
    assert!(!context.contains(root.to_str().unwrap()));

    root_request(
        "write",
        root,
        json!({"operation":"commit","paths":["新目录/选中 @.txt"],"message":"selected only"}),
    );
    assert_eq!(
        svn(
            root,
            &[
                "cat",
                &format!(
                    "{}/trunk/%E6%96%B0%E7%9B%AE%E5%BD%95/%E9%80%89%E4%B8%AD%20%40.txt@",
                    fixture.url
                )
            ]
        ),
        "one\n"
    );
    assert!(
        entries(root)
            .iter()
            .any(|entry| entry["path"] == "新目录/留在本地.txt" && entry["item"] == "added")
    );
    assert!(
        entries(root)
            .iter()
            .any(|entry| entry["path"] == "unchanged.txt" && entry["item"] == "modified")
    );
    assert_eq!(
        svn(
            root,
            &["cat", &format!("{}/trunk/unchanged.txt", fixture.url)]
        ),
        "leave this file alone\n"
    );
    let outside = handle(
        "write",
        json!({"operation":"delete","root":root,"paths":["embedded/text.txt"]}),
        &Cancellation::default(),
    )
    .unwrap_err();
    assert_eq!(outside.kind, "invalid_params");

    root_request(
        "write",
        root,
        json!({"operation":"move","path":"新目录/选中 @.txt","destination":"新目录/改名 @.txt"}),
    );
    let move_entries = entries(root);
    assert!(
        move_entries
            .iter()
            .any(|entry| entry["path"] == "新目录/改名 @.txt" && entry["item"] == "added"),
        "{move_entries:?}"
    );
    let partial_move = handle(
        "read",
        json!({"root":root,"operation":"commit_diff","paths":["新目录/改名 @.txt"]}),
        &Cancellation::default(),
    )
    .unwrap_err();
    assert_eq!(partial_move.kind, "invalid_params");
    let context = root_request(
        "read",
        root,
        json!({"operation":"commit_diff","paths":["新目录/选中 @.txt","新目录/改名 @.txt"]}),
    );
    assert!(
        context["text"]
            .as_str()
            .unwrap()
            .contains("moved 新目录/选中 @.txt -> 新目录/改名 @.txt")
    );
    root_request(
        "write",
        root,
        json!({"operation":"commit","paths":["新目录/选中 @.txt","新目录/改名 @.txt"],"message":"move selected file"}),
    );
    assert!(root.join("新目录/改名 @.txt").is_file());
}

#[test]
fn reads_and_repository_operations_are_real() {
    let fixture = Repos::new();
    let root = &fixture.a;
    let log = root_request(
        "read",
        root,
        json!({"operation":"history","path":"text.txt"}),
    );
    assert!(log["text"].as_str().unwrap().contains("seed"));
    let blame = root_request("read", root, json!({"operation":"blame","path":"text.txt"}));
    assert!(blame["text"].as_str().unwrap().contains("original"));
    assert_eq!(blame["blame"]["content"], "original\nline two\n");
    assert_eq!(blame["blame"]["lines"][0]["line"], 1);
    assert_eq!(blame["blame"]["lines"][0]["revision"], "2");
    root_request(
        "write",
        root,
        json!({"operation":"set_property","path":"text.txt","name":"qa","value":"行 1\n行 2"}),
    );
    let properties = root_request(
        "read",
        root,
        json!({"operation":"properties","path":"text.txt"}),
    );
    assert!(properties["text"].as_str().unwrap().contains("行 2"));
    root_request(
        "write",
        root,
        json!({"operation":"changelist","paths":["text.txt"],"name":"review"}),
    );
    assert!(
        entries(root)
            .iter()
            .any(|entry| entry["changelist"] == "review")
    );
    root_request(
        "write",
        root,
        json!({"operation":"changelist","paths":["text.txt"],"name":null}),
    );
    root_request(
        "write",
        root,
        json!({"operation":"set_property","path":"text.txt","name":"qa","value":null}),
    );
    assert!(!svn(root, &["proplist", "text.txt"]).contains("qa"));
    root_request("write", root, json!({"operation":"cleanup"}));
    root_request(
        "write",
        root,
        json!({"operation":"revert","paths":["text.txt"]}),
    );
    root_request(
        "write",
        root,
        json!({"operation":"lock","paths":["text.txt"],"message":"qa lock"}),
    );
    assert!(svn(root, &["info", "text.txt"]).contains("Lock Token"));
    let lock_status = |copy: &Path, remote| {
        let status = root_request("status", copy, json!({"remote":remote}));
        status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["path"] == "text.txt")
            .expect("locked file in status")
            .clone()
    };
    let local = lock_status(root, false);
    assert_eq!(local["locked"], true);
    assert_eq!(local["lock"]["comment"], "qa lock");
    assert_eq!(local["lock"]["held_here"], true);
    assert!(
        local["lock"]["owner"]
            .as_str()
            .is_some_and(|owner| !owner.is_empty())
    );
    assert!(local["lock"]["token"].is_null());
    assert_eq!(lock_status(root, true)["lock"], local["lock"]);
    let other = lock_status(&fixture.b, true);
    assert_eq!(other["locked"], true);
    assert_eq!(other["lock"]["owner"], local["lock"]["owner"]);
    assert_eq!(other["lock"]["comment"], "qa lock");
    assert_eq!(other["lock"]["held_here"], false);
    root_request(
        "write",
        root,
        json!({"operation":"unlock","paths":["text.txt"]}),
    );
    assert!(!svn(root, &["info", "text.txt"]).contains("Lock Token"));

    let trunk = format!("{}/trunk", fixture.url);
    let branch = format!("{}/branches/feature", fixture.url);
    root_request(
        "write",
        root,
        json!({"operation":"copy_branch","source":trunk,"destination":branch,"message":"create branch"}),
    );
    let branches = root_request(
        "read",
        root,
        json!({"operation":"branches","url":format!("{}/branches",fixture.url)}),
    );
    assert!(branches["text"].as_str().unwrap().contains("feature/"));
    let tag = format!("{}/tags/v1%20release", fixture.url);
    root_request(
        "write",
        root,
        json!({"operation":"copy_branch","source":trunk,"destination":tag,"message":"create tag"}),
    );
    let targets = root_request("read", root, json!({"operation":"switch_targets"}));
    assert_eq!(
        targets["targets"],
        json!([
            {"path":"trunk","url":trunk},
            {"path":"branches/feature","url":branch},
            {"path":"tags/v1 release","url":tag}
        ])
    );
    root_request(
        "write",
        root,
        json!({"operation":"switch","url":targets["targets"][1]["url"]}),
    );
    assert_eq!(
        svn(&fixture.b, &["info", "--show-item", "url"]).trim(),
        trunk
    );
    fs::write(root.join("text.txt"), "branch edit\nline two\n").unwrap();
    root_request(
        "write",
        root,
        json!({"operation":"commit","paths":["text.txt"],"message":"branch edit"}),
    );
    root_request("write", root, json!({"operation":"switch","url":trunk}));
    let dry = root_request(
        "write",
        root,
        json!({"operation":"merge","source":branch,"dry_run":true}),
    );
    assert!(dry["text"].as_str().unwrap().contains("text.txt"));
    assert_eq!(
        fs::read_to_string(root.join("text.txt")).unwrap(),
        "original\nline two\n"
    );
    root_request("write", root, json!({"operation":"merge","source":branch}));
    assert_eq!(
        fs::read_to_string(root.join("text.txt")).unwrap(),
        "branch edit\nline two\n"
    );
    let diff = root_request("read", root, json!({"operation":"diff","path":"text.txt"}));
    assert!(diff["text"].as_str().unwrap().contains("+branch edit"));
}

#[test]
fn update_and_resolve_text_property_and_tree_conflicts() {
    let fixture = Repos::new();
    let (a, b) = (&fixture.a, &fixture.b);
    fs::write(a.join("text.txt"), "theirs\nline two\n").unwrap();
    fs::write(b.join("text.txt"), "mine\nline two\n").unwrap();
    svn(a, &["propset", "qa", "theirs", "unchanged.txt"]);
    svn(b, &["propset", "qa", "mine", "unchanged.txt"]);
    fs::write(a.join("new.txt"), "theirs addition").unwrap();
    fs::write(b.join("new.txt"), "mine addition").unwrap();
    svn(a, &["add", "new.txt"]);
    svn(b, &["add", "new.txt"]);
    svn(a, &["commit", "-m", "incoming"]);
    root_request("write", b, json!({"operation":"update"}));
    fs::write(b.join("legitimate.mine"), "keep this untracked file").unwrap();
    let before = entries(b);
    let untracked = before
        .iter()
        .filter(|entry| entry["item"] == "unversioned")
        .filter_map(|entry| entry["path"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(untracked, ["legitimate.mine"]);
    assert!(
        before
            .iter()
            .any(|e| e["path"] == "text.txt" && e["item"] == "conflicted"),
        "{before:?}"
    );
    assert!(
        before
            .iter()
            .any(|e| e["path"] == "unchanged.txt" && e["props"] == "conflicted"),
        "{before:?}"
    );
    assert!(
        before
            .iter()
            .any(|e| e["path"] == "new.txt" && e["tree_conflicted"] == true),
        "{before:?}"
    );

    let comparison = root_request("read", b, json!({"operation":"conflict","path":"text.txt"}));
    assert!(
        comparison["views"]["base"]
            .as_str()
            .unwrap_or_default()
            .contains("original"),
        "{comparison}"
    );
    assert!(
        comparison["views"]["mine"]
            .as_str()
            .unwrap_or_default()
            .contains("mine"),
        "{comparison}"
    );
    assert!(
        comparison["views"]["incoming"]
            .as_str()
            .unwrap_or_default()
            .contains("theirs"),
        "{comparison}"
    );
    let properties = root_request(
        "read",
        b,
        json!({"operation":"conflict","path":"unchanged.txt"}),
    );
    assert_eq!(properties["views"]["property_base"], "qa = \n");
    assert_eq!(properties["views"]["property_mine"], "qa = mine\n");
    assert_eq!(properties["views"]["property_incoming"], "qa = theirs\n");
    let tree = root_request("read", b, json!({"operation":"conflict","path":"new.txt"}));
    assert_eq!(tree["tree"]["action"], "add");
    assert_eq!(tree["tree"]["reason"], "add");
    assert_eq!(tree["tree"]["operation"], "update");
    assert_eq!(tree["tree"]["incoming"]["path"], "trunk/new.txt");
    assert_eq!(tree["views"]["working"], "mine addition");
    assert_eq!(tree["views"]["tree_incoming"], "theirs addition");
    let incoming_revision = tree["tree_incoming_revision"].as_u64().unwrap();

    let unresolved = handle(
        "write",
        json!({
            "root":b,"operation":"resolve","path":"text.txt","accept":"working","kind":"text",
        }),
        &Cancellation::default(),
    )
    .unwrap_err();
    assert!(unresolved.message.contains("conflict markers"));
    let stale = handle(
        "write",
        json!({
            "root":b,"operation":"resolve","path":"new.txt","accept":"theirs-full",
            "kind":"tree","incoming_revision":incoming_revision + 1,"expected_working":"mine addition",
        }),
        &Cancellation::default(),
    )
    .unwrap_err();
    assert!(stale.message.contains("changed"));
    assert_eq!(
        fs::read_to_string(b.join("new.txt")).unwrap(),
        "mine addition"
    );
    fs::write(b.join("text.txt"), "merged by user\n").unwrap();
    for (path, accept, kind) in [
        ("text.txt", "working", "text"),
        ("unchanged.txt", "theirs-full", "property"),
        ("new.txt", "theirs-full", "tree"),
    ] {
        root_request(
            "write",
            b,
            json!({
                "operation":"resolve","path":path,"accept":accept,"kind":kind,
                "incoming_revision":incoming_revision,"expected_working":"mine addition",
            }),
        );
    }
    assert!(entries(b).iter().all(|entry| entry["item"] != "conflicted"
        && entry["props"] != "conflicted"
        && entry["tree_conflicted"] == false));
    assert_eq!(
        fs::read_to_string(b.join("text.txt")).unwrap(),
        "merged by user\n"
    );
    assert_eq!(
        svn(b, &["propget", "--strict", "qa", "unchanged.txt"]),
        "theirs"
    );
    assert_eq!(
        fs::read_to_string(b.join("new.txt")).unwrap(),
        "theirs addition"
    );
}

#[test]
fn provider_handshake_and_cancellation_use_stdio() {
    use std::io::{BufRead, BufReader, Write};
    let mut child = Command::new(env!("CARGO_BIN_EXE_wu-plus-subversion-provider"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})
    )
    .unwrap();
    let mut reply = String::new();
    output.read_line(&mut reply).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap()["result"]["protocol_version"],
        1
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let tmp = TempDir::new().unwrap();
    let addr = listener.local_addr().unwrap();
    writeln!(input, "{}", json!({
        "jsonrpc":"2.0","id":2,"method":"write",
        "params":{"operation":"checkout","url":format!("svn://{addr}/stall"),"destination":tmp.path().join("checkout")}
    })).unwrap();
    let start = std::time::Instant::now();
    let _stalled_connection = loop {
        match listener.accept() {
            Ok((connection, _)) => break connection,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && start.elapsed().as_secs() < 8 =>
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => panic!("SVN did not connect: {error}"),
        }
    };
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":2}})
    )
    .unwrap();
    reply.clear();
    output.read_line(&mut reply).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap()["error"]["data"]["kind"],
        "cancelled",
        "{reply}"
    );
    drop(input);
    assert!(child.wait().unwrap().success());
}

#[test]
fn svnserve_authentication_failure_and_password_stdin() {
    let fixture = Repos::new();
    let server = fixture.tmp.path().join("repository");
    fs::write(server.join("conf/svnserve.conf"), "[general]\nanon-access = none\nauth-access = write\npassword-db = passwd\nrealm = wu-plus-subversion-tests\n").unwrap();
    fs::write(
        server.join("conf/passwd"),
        "[users]\nfixture-user = fixture-password\n",
    )
    .unwrap();

    use std::net::{TcpListener, TcpStream};
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut svnserve = Command::new("svnserve")
        .args([
            "-d",
            "--foreground",
            "--listen-host",
            "127.0.0.1",
            "--listen-port",
            &port.to_string(),
            "-r",
        ])
        .arg(&server)
        .spawn()
        .unwrap();
    struct OnDrop<'a>(&'a mut std::process::Child);
    impl Drop for OnDrop<'_> {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _server = OnDrop(&mut svnserve);
    let start = std::time::Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(start.elapsed().as_secs() < 8, "svnserve failed to start");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let url = format!("svn://127.0.0.1:{port}/trunk");
    let destination = fixture.tmp.path().join("authenticated");
    let wrong = handle("write", json!({"operation":"checkout","url":url,"destination":destination,"auth":{"username":"fixture-user","password":"wrong-password"}}), &Cancellation::default()).unwrap_err();
    assert_eq!(wrong.kind, "authentication", "{}", wrong.message);
    assert!(!wrong.message.contains("wrong-password"));
    request(
        "write",
        json!({"operation":"checkout","url":url,"destination":destination,"auth":{"username":"fixture-user","password":"fixture-password"}}),
    );
    assert_eq!(
        fs::read_to_string(destination.join("text.txt")).unwrap(),
        "original\nline two\n"
    );
}

#[test]
fn directory_externals_are_listed_as_separate_working_copies() {
    let fixture = Repos::new();
    svn(
        &fixture.a,
        &[
            "propset",
            "svn:externals",
            "^/trunk external-copy\n^/trunk/text.txt external-file.txt",
            ".",
        ],
    );
    root_request(
        "write",
        &fixture.a,
        json!({"operation":"update","with_externals":true}),
    );
    let found = request("discover", json!({"roots":[fixture.a]}));
    assert_eq!(found["copies"].as_array().unwrap().len(), 2, "{found}");
    let external = fixture.a.join("external-copy");
    fs::write(external.join("text.txt"), "external local change\n").unwrap();
    assert!(
        entries(&external)
            .iter()
            .any(|entry| entry["path"] == "text.txt" && entry["item"] == "modified")
    );
    assert!(
        !entries(&fixture.a)
            .iter()
            .any(|entry| entry["path"] == "external-copy/text.txt")
    );
    fs::write(
        fixture.a.join("external-file.txt"),
        "local file external change\n",
    )
    .unwrap();
    let parent_entries = entries(&fixture.a);
    assert!(
        parent_entries
            .iter()
            .any(|entry| entry["path"] == "external-file.txt" && entry["file_external"] == true),
        "{parent_entries:?}"
    );
    let result = handle("write", json!({
        "root":fixture.a,"operation":"commit","paths":["external-file.txt"],"message":"outside the working copy"
    }), &Cancellation::default()).unwrap_err();
    assert_eq!(result.kind, "invalid_params");
}

#[test]
fn native_diff_snapshots_and_history_file_changes() {
    let fixture = Repos::new();
    let root = &fixture.a;
    let initial = (1..=40)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    fs::write(root.join("text.txt"), &initial).unwrap();
    let seed = root_request(
        "write",
        root,
        json!({"operation":"commit","paths":["text.txt"],"message":"Prepare distant changes"}),
    );
    assert!(seed["text"].as_str().unwrap().contains("revision 3"));
    let working = initial
        .replace("line 2\n", "new first chunk\n")
        .replace("line 38\n", "new second chunk\n");
    fs::write(root.join("text.txt"), &working).unwrap();
    let baseline = root_request(
        "read",
        root,
        json!({"operation":"editor_base","path":"text.txt"}),
    );
    assert_eq!(baseline["base"], initial);
    let blame = root_request(
        "read",
        root,
        json!({"operation":"blame","path":"text.txt","editor":true}),
    );
    assert_eq!(blame["text"], "");
    assert_eq!(blame["blame"]["content"], working);
    let spans = blame["blame"]["lines"].as_array().unwrap();
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0]["line"], 1);
    assert_eq!(spans[0]["end_line"], 1);
    assert_eq!(spans[1]["line"], 3);
    assert_eq!(spans[1]["end_line"], 37);
    assert_eq!(spans[2]["line"], 39);
    assert_eq!(spans[2]["end_line"], 40);
    let diff = root_request("read", root, json!({"operation":"diff","path":"text.txt"}));
    assert_eq!(diff["file"]["base"], initial);
    assert_eq!(diff["file"]["head"], working);
    assert_eq!(diff["file"]["from"], "BASE");
    assert_eq!(diff["file"]["to"], "WORKING");
    assert_eq!(
        diff["text"]
            .as_str()
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("@@"))
            .count(),
        2
    );

    let name = "中文 @.txt";
    fs::write(root.join(name), "added content\n").unwrap();
    root_request("write", root, json!({"operation":"add","paths":[name]}));
    let baseline = root_request("read", root, json!({"operation":"editor_base","path":name}));
    assert_eq!(baseline["base"], "");
    let diff = root_request("read", root, json!({"operation":"diff","path":name}));
    assert_eq!(diff["file"]["base"], "");
    assert_eq!(diff["file"]["head"], "added content\n");
    fs::write(root.join("binary.dat"), [0, 1, 2]).unwrap();
    let baseline = root_request(
        "read",
        root,
        json!({"operation":"editor_base","path":"binary.dat"}),
    );
    assert!(baseline["base"].is_null());
    let overview = root_request("read", root, json!({"operation":"worktree_diff"}));
    let focused = root_request(
        "read",
        root,
        json!({"operation":"worktree_diff","focus_path":"text.txt"}),
    );
    assert_eq!(focused["other"], json!([]));
    assert_eq!(focused["limited"], false);
    assert_eq!(
        focused["files"],
        json!([overview["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == "text.txt")
            .unwrap()])
    );
    let focused = root_request(
        "read",
        root,
        json!({"operation":"worktree_diff","focus_path":name}),
    );
    assert_eq!(focused["files"][0]["path"], name);
    assert_eq!(focused["files"][0]["base"], "");
    let files = overview["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(
        files
            .iter()
            .any(|file| file["path"] == name && file["base"] == "")
    );
    assert!(
        files
            .iter()
            .any(|file| file["path"] == "text.txt" && file["head"] == working)
    );
    assert_eq!(overview["other"], json!(["binary.dat"]));
    assert_eq!(overview["limited"], false);
    fs::write(root.join("new notes.md"), "one\ntwo\n").unwrap();
    let status = root_request("status", root, json!({"diff_stats":true}));
    let stats = |path: &str| {
        &status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["path"] == path)
            .unwrap()["diff_stat"]
    };
    assert_eq!(*stats("text.txt"), json!({"added":2,"deleted":2}));
    assert_eq!(*stats(name), json!({"added":1,"deleted":0}));
    assert_eq!(*stats("new notes.md"), json!({"added":2,"deleted":0}));
    assert!(stats("binary.dat").is_null());
    root_request(
        "write",
        root,
        json!({"operation":"commit","paths":[name,"text.txt"],"message":"Save two chunks and a new file"}),
    );

    let history = root_request("read", root, json!({"operation":"history","path":"."}));
    let latest = &history["revisions"][0];
    assert_eq!(latest["revision"], 4);
    assert_eq!(latest["message"], "Save two chunks and a new file");
    assert!(latest["paths"].as_array().unwrap().iter().any(|entry| {
        entry["path"] == format!("/trunk/{name}")
            && entry["action"] == "A"
            && entry["kind"] == "file"
    }));
    let historical = root_request(
        "read",
        root,
        json!({"operation":"revision_file","path":"/trunk/text.txt","revision":4,"action":"M"}),
    );
    assert_eq!(historical["file"]["base"], initial);
    assert_eq!(historical["file"]["head"], working);
    assert_eq!(historical["file"]["from"], "r3");
    assert_eq!(historical["file"]["to"], "r4");
    let stats = root_request(
        "read",
        root,
        json!({"operation":"revision_review","revision":4}),
    );
    assert_eq!(stats["stats"], json!({"added":3,"deleted":2}));
    assert!(stats["files"].is_null());
    let review = root_request(
        "read",
        root,
        json!({"operation":"revision_review","revision":4,"include_files":true}),
    );
    assert_eq!(review["stats"], stats["stats"]);
    assert_eq!(review["message"], latest["message"]);
    assert_eq!(review["author"], latest["author"]);
    assert_eq!(review["date"], latest["date"]);
    assert_eq!(review["files"].as_array().unwrap().len(), 2);
    assert!(review["files"].as_array().unwrap().iter().any(|file| {
        file["path"] == "trunk/text.txt" && file["base"] == initial && file["head"] == working
    }));
    let focused_review = root_request(
        "read",
        root,
        json!({
            "operation":"revision_review","revision":4,"include_files":true,
            "focus_path":"/trunk/text.txt"
        }),
    );
    assert_eq!(
        focused_review["files"],
        json!([review["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == "trunk/text.txt")
            .unwrap()])
    );
    assert_eq!(focused_review["other"], json!([]));
    assert_eq!(focused_review["message"], review["message"]);
    assert_eq!(focused_review["author"], review["author"]);
    assert_eq!(focused_review["date"], review["date"]);
    assert!(focused_review["stats"].is_null());
    let layout = root_request(
        "read",
        root,
        json!({"operation":"revision_review","revision":1,"include_files":true}),
    );
    assert_eq!(layout["stats"], json!({"added":0,"deleted":0}));
    assert_eq!(layout["files"].as_array().unwrap().len(), 3);

    let added = root_request(
        "read",
        root,
        json!({"operation":"revision_file","path":format!("/trunk/{name}"),"revision":4,"action":"A"}),
    );
    assert_eq!(added["file"]["base"], "");
    assert_eq!(added["file"]["head"], "added content\n");
    root_request("write", root, json!({"operation":"delete","paths":[name]}));
    let deleted = root_request("read", root, json!({"operation":"diff","path":name}));
    assert_eq!(deleted["file"]["base"], "added content\n");
    assert_eq!(deleted["file"]["head"], "");
    root_request(
        "write",
        root,
        json!({"operation":"commit","paths":[name],"message":"Delete the special filename"}),
    );
    let deleted = root_request(
        "read",
        root,
        json!({"operation":"revision_file","path":format!("/trunk/{name}"),"revision":5,"action":"D"}),
    );
    assert_eq!(deleted["file"]["base"], "added content\n");
    assert_eq!(deleted["file"]["head"], "");

    root_request(
        "write",
        root,
        json!({"operation":"set_property","path":"text.txt","name":"qa","value":"review-ok"}),
    );
    let prop_diff = root_request("read", root, json!({"operation":"diff","path":"text.txt"}));
    assert!(prop_diff["file"].is_null());
    assert_eq!(prop_diff["details"]["kind"], "properties");
    assert_eq!(prop_diff["details"]["base"], "");
    assert_eq!(prop_diff["details"]["head"], "qa = review-ok\n");
    let overview = root_request("read", root, json!({"operation":"worktree_diff"}));
    assert!(
        overview["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| { file["path"] == "text.txt" && file["kind"] == "properties" })
    );

    fs::write(root.join("text.txt"), "content and properties\n").unwrap();
    let combined = root_request("read", root, json!({"operation":"diff","path":"text.txt"}));
    assert_eq!(combined["file"]["head"], "content and properties\n");
    assert_eq!(combined["details"]["head"], "qa = review-ok\n");
    let summary = root_request(
        "read",
        root,
        json!({"operation":"commit_diff","paths":["text.txt"]}),
    );
    let summary = summary["text"].as_str().unwrap();
    assert!(summary.contains("+content and properties"));
    assert!(summary.contains("qa"));
    assert!(summary.contains("review-ok"));
    assert!(!summary.contains("binary.dat"));
    fs::write(
        root.join("text.txt"),
        "多行说明
"
        .repeat(5000),
    )
    .unwrap();
    let limited = root_request(
        "read",
        root,
        json!({"operation":"commit_diff","paths":["text.txt"]}),
    );
    let limited = limited["text"].as_str().unwrap();
    assert!(limited.len() <= 20_000);
    assert!(limited.ends_with("[SVN diff truncated]\n"));
    fs::write(root.join("text.txt"), "content and properties\n").unwrap();

    root_request(
        "write",
        root,
        json!({"operation":"commit","paths":["text.txt"],"message":"Save content and properties"}),
    );
    let historical = root_request(
        "read",
        root,
        json!({"operation":"revision_file","path":"/trunk/text.txt","revision":6,"action":"M"}),
    );
    assert_eq!(historical["file"]["head"], "content and properties\n");
    assert_eq!(historical["details"]["head"], "qa = review-ok\n");

    let moved = "renamed @.txt";
    root_request(
        "write",
        root,
        json!({"operation":"move","path":"text.txt","destination":moved}),
    );
    for path in ["text.txt", moved] {
        let diff = root_request("read", root, json!({"operation":"diff","path":path}));
        assert!(diff["file"].is_null(), "{diff:?}");
        assert_eq!(diff["details"]["kind"], "path");
        assert_eq!(diff["details"]["base"], "text.txt\n");
        assert_eq!(diff["details"]["head"], format!("{moved}\n"));
    }
    fs::write(root.join(moved), "edited after move\n").unwrap();
    let changed = root_request("read", root, json!({"operation":"diff","path":"text.txt"}));
    assert_eq!(changed["file"]["path"], moved);
    assert_eq!(changed["file"]["base"], "content and properties\n");
    assert_eq!(changed["file"]["head"], "edited after move\n");

    fs::create_dir(root.join("new-folder")).unwrap();
    root_request(
        "write",
        root,
        json!({"operation":"add","paths":["new-folder"]}),
    );
    let directory = root_request(
        "read",
        root,
        json!({"operation":"diff","path":"new-folder"}),
    );
    assert_eq!(directory["details"]["head"], "new-folder\n");
    let binary = root_request(
        "read",
        root,
        json!({"operation":"diff","path":"binary.dat"}),
    );
    assert_eq!(binary["reason"], "binary");
    assert!(binary["file"].is_null());
}

#[test]
fn register_missing_delete_and_resolve_head_revision_diff() {
    let fixture = Repos::new();
    let root = &fixture.a;
    fs::remove_file(root.join("unchanged.txt")).unwrap();
    let missing = root_request(
        "read",
        root,
        json!({"operation":"diff","path":"unchanged.txt"}),
    );
    assert_eq!(missing["file"]["base"], "leave this file alone\n");
    assert_eq!(missing["file"]["head"], "");
    root_request(
        "write",
        root,
        json!({"operation":"delete","paths":["unchanged.txt"]}),
    );
    assert!(
        entries(root)
            .iter()
            .any(|entry| entry["path"] == "unchanged.txt" && entry["item"] == "deleted")
    );
    root_request(
        "write",
        root,
        json!({"operation":"revert","paths":["unchanged.txt"]}),
    );
    assert!(root.join("unchanged.txt").is_file());

    fs::write(root.join("text.txt"), "committed at head\n").unwrap();
    root_request(
        "write",
        root,
        json!({"operation":"commit","paths":["text.txt"],"message":"HEAD content"}),
    );
    let diff = root_request(
        "read",
        root,
        json!({"operation":"diff","path":"text.txt","revision":"HEAD"}),
    );
    assert_eq!(diff["file"]["from"], "r2");
    assert_eq!(diff["file"]["to"], "r3");
    assert_eq!(diff["file"]["head"], "committed at head\n");
}
