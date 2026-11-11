mod binding_arrays;
mod buffer;
mod buffer_mapping;
mod buffer_slice;
mod command_buffer_actions;
mod device;
mod encoding;
mod error_scopes;
mod experimental;
mod external_texture;
mod immediates;
mod instance;
mod mesh_shader;
mod overrides;
mod ray_tracing_invocation_reorder;
mod ray_tracing_pipeline;
mod render_bundle;
mod render_pipeline;
#[cfg(not(target_family = "wasm"))] // tests share wgpu objects across threads
mod surface;
mod texture;
#[cfg(not(target_family = "wasm"))] // the callbacks need `wgpu` objects to be `Send`
mod user_callbacks;
