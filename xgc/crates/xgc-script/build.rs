fn main() {
    println!("cargo:rerun-if-changed=src/instruction_hook.c");
    cc::Build::new()
        .file("src/instruction_hook.c")
        .compile("xgc_lua_instruction_hook");
}
