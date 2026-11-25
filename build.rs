fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/disqro.rc");
        println!("cargo:rerun-if-changed=assets/brand/disqro.ico");
        embed_resource::compile("assets/disqro.rc", embed_resource::NONE)
            .manifest_required()
            .expect("Windows application resources");
        waveform_shaders();
    }
}

#[cfg(windows)]
fn waveform_shaders() {
    use std::{env, fs, path::PathBuf, slice};
    use windows::{
        Win32::Graphics::Direct3D::Fxc::{
            D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile,
        },
        core::{PCSTR, s},
    };

    let path = "src/waveformGpu/waveform.hlsl";
    println!("cargo:rerun-if-changed={path}");
    let source = fs::read(path).expect("read the waveform shader");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo shader output directory"));
    for (entry, target, name) in [
        (s!("vertex"), s!("vs_5_0"), "waveform.vs"),
        (s!("fragment"), s!("ps_5_0"), "waveform.ps"),
        (s!("stroke_cores"), s!("cs_5_0"), "strokeCores.cs"),
        (s!("stroke_columns"), s!("cs_5_0"), "strokeColumns.cs"),
    ] {
        let mut code = None;
        let mut errors = None;
        // SAFETY: the source slice and entry strings outlive compilation and
        // the output blobs own their bytecode and diagnostics.
        unsafe {
            if let Err(error) = D3DCompile(
                source.as_ptr().cast(),
                source.len(),
                PCSTR::null(),
                None,
                None,
                entry,
                target,
                D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut code,
                Some(&mut errors),
            ) {
                let detail = errors.as_ref().map(|blob| {
                    String::from_utf8_lossy(slice::from_raw_parts(
                        blob.GetBufferPointer().cast(),
                        blob.GetBufferSize(),
                    ))
                });
                panic!(
                    "compile {name}: {error} {}",
                    detail.as_deref().unwrap_or_default()
                );
            }
            let code = code.expect("D3DCompile returned no bytecode");
            fs::write(
                output.join(name),
                slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()),
            )
            .expect("store waveform shader bytecode");
        }
    }
}
