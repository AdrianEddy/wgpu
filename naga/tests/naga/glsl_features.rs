/*!
Test that [`Features::unsupported_at`] reports the GLSL backend's version gates as its writer
applies them.
*/

#![cfg(all(feature = "wgsl-in", glsl_out))]

use naga::back::glsl::{self, Features, Version};

/// A compute shader with a runtime-sized storage array, the shape of wgpu-core's indirect-call
/// validation shaders.
const COMPUTE_WITH_RUNTIME_ARRAY: &str = "
@group(0) @binding(0) var<storage, read_write> data: array<u32>;

@compute @workgroup_size(1)
fn main() {
    data[0] = arrayLength(&data);
}
";

fn write(source: &str, version: Version) -> Result<String, glsl::Error> {
    let module = naga::front::wgsl::parse_str(source).unwrap();
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
    let options = glsl::Options {
        version,
        ..Default::default()
    };
    let pipeline_options = glsl::PipelineOptions {
        shader_stage: naga::ShaderStage::Compute,
        entry_point: "main".into(),
        multiview: None,
    };
    let mut out = String::new();
    glsl::Writer::new(
        &mut out,
        &module,
        &info,
        &options,
        &pipeline_options,
        Default::default(),
    )?
    .write()?;
    Ok(out)
}

#[test]
fn unsupported_at_reports_what_the_writer_refuses() {
    let needed = Features::COMPUTE_SHADER | Features::BUFFER_STORAGE | Features::DYNAMIC_ARRAY_SIZE;

    // The version NVIDIA grants for a GL 3.3 core context: none of the three.
    assert_eq!(needed.unsupported_at(Version::Desktop(330)), needed);

    for version in [
        Version::Desktop(330),
        Version::Desktop(400),
        Version::Desktop(410),
        Version::Desktop(420),
        Version::Desktop(430),
        Version::Desktop(450),
        Version::new_gles(300),
        Version::new_gles(310),
        Version::new_gles(320),
    ] {
        let missing = needed.unsupported_at(version);
        match write(COMPUTE_WITH_RUNTIME_ARRAY, version) {
            Ok(_) => assert!(
                missing.is_empty(),
                "{version}: the writer wrote it, but `unsupported_at` reports {missing:?}"
            ),
            Err(glsl::Error::MissingFeatures(refused)) => {
                assert!(
                    !missing.is_empty(),
                    "{version}: the writer refused {refused:?}"
                );
                assert_eq!(refused, missing, "{version}");
            }
            Err(e) => panic!("{version}: {e}"),
        }
    }
}
