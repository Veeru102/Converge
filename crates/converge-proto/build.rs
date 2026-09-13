fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    std::env::set_var("PROTOC", protoc);
    prost_build::Config::new()
        .compile_protos(
            &["../../proto/converge/v1/protocol.proto"],
            &["../../proto"],
        )
        .expect("compile protocol.proto");
    println!("cargo:rerun-if-changed=../../proto/converge/v1/protocol.proto");
}
