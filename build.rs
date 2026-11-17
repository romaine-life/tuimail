//! The tests against a real mail server (`live.rs`) run where GreenMail is: the CI suite's
//! service job, or a local run that names its certificate (TESTING.md).

fn main() {
    println!("cargo::rustc-check-cfg=cfg(greenmail)");
    println!("cargo::rerun-if-env-changed=GREENMAIL_CERT");
    if std::env::var_os("GREENMAIL_CERT").is_some() {
        println!("cargo::rustc-cfg=greenmail");
    }
}
