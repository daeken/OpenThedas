//! Prints the adapter's name and the features the UI renderer may use.
fn main() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter =
        pollster::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let f = adapter.features();
    let l = adapter.limits();
    println!("{:?}", adapter.get_info());
    for (n, x) in [
        (
            "TEXTURE_BINDING_ARRAY",
            wgpu::Features::TEXTURE_BINDING_ARRAY,
        ),
        (
            "NON_UNIFORM_INDEXING",
            wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
        ),
        (
            "PARTIALLY_BOUND",
            wgpu::Features::PARTIALLY_BOUND_BINDING_ARRAY,
        ),
        ("BC", wgpu::Features::TEXTURE_COMPRESSION_BC),
        (
            "MULTI_DRAW_INDIRECT_COUNT",
            wgpu::Features::MULTI_DRAW_INDIRECT_COUNT,
        ),
        (
            "INDIRECT_FIRST_INSTANCE",
            wgpu::Features::INDIRECT_FIRST_INSTANCE,
        ),
    ] {
        println!("{n}: {}", f.contains(x));
    }
    println!(
        "max sampled textures per stage {}, max binding array elements {:?}, storage buffer {}",
        l.max_sampled_textures_per_shader_stage,
        l.max_binding_array_elements_per_shader_stage,
        l.max_storage_buffer_binding_size
    );
}
