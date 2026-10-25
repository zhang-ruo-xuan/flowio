use std::env;

fn main() {
    tauri_build::build();
    println!("cargo:rerun-if-changed=../dist");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities");

    // 🔴 r1030 安全修复：内置免费通道的模型 Key 改为**构建期注入**
    //   （此前是明文硬编码在 Rust 源码里，而本仓是 GitHub 公开仓 ⇒ 任何人
    //   clone 下来就能拿走并盗用你的免费额度）。
    //
    //   · 源码与 git 历史里不再有明文 Key；
    //   · 发布者在自己机器上注入环境变量即可产出带 Key 的 exe：
    //       FLOWIO_BUILTIN_AI_KEY=... FLOWIO_BUILTIN_AI_FALLBACK_KEY=... cargo build --release
    //   · 没注入也能编译（不 panic，避免挡住 cargo test / CI）：
    //     源码侧用 option_env! 读不到就是 None ⇒ 内置通道判定为「未配置」，
    //     UI 会明确提示用户去设置里填自己的 Key（而不是静默发出 401）。
    //
    //   🔴 改了这两个环境变量必须重新编译才能生效（rerun-if-env-changed）。
    println!("cargo:rerun-if-env-changed=FLOWIO_BUILTIN_AI_KEY");
    println!("cargo:rerun-if-env-changed=FLOWIO_BUILTIN_AI_FALLBACK_KEY");
    println!("cargo:rerun-if-env-changed=FLOWIO_BUILTIN_DASHSCOPE_KEY");

    // 编译期把值写进环境（供源码里的 option_env! 读取）。
    // 之所以在这里显式 set：option_env! 读的是**编译进程**的环境，
    // 而 cargo 已经把 env 传给 rustc 了；显式 set 只是保证 rerun 语义清晰。
    for key in [
        "FLOWIO_BUILTIN_AI_KEY",
        "FLOWIO_BUILTIN_AI_FALLBACK_KEY",
        "FLOWIO_BUILTIN_DASHSCOPE_KEY",
    ] {
        let v = env::var(key).unwrap_or_default();
        if !v.trim().is_empty() {
            // 只在有值时 set，避免给 rustc 塞空串
            unsafe {
                env::set_var(key, &v);
            }
        }
    }
}
