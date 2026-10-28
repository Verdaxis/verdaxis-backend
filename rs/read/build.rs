fn main() {
    println!("cargo:rerun-if-env-changed=RELEASE_SHA");
    let sha = std::env::var("RELEASE_SHA").expect("build requires RELEASE_SHA");
    assert!(
        sha.len() == 40
            && sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert!(sha.bytes().any(|b| b != b'0'));
    println!("cargo:rustc-env=VERDAXIS_BUILD_SHA={sha}");
}
