export function getBrowserSettings(config='intel-coherent'){
 const env={...process.env};
 if(config==='intel-coherent')for(const key of ['__NV_PRIME_RENDER_OFFLOAD','__GLX_VENDOR_LIBRARY_NAME','__VK_LAYER_NV_optimus','__EGL_VENDOR_LIBRARY_FILENAMES','VK_ICD_FILENAMES'])delete env[key];
 const extra=config==='intel-coherent'?['--enable-unsafe-webgpu','--ignore-gpu-blocklist','--disable-software-rasterizer','--enable-features=Vulkan,WebGPUOnVulkanViaGLInterop','--use-angle=gl']:config==='nvidia-vulkan'?['--use-angle=vulkan','--enable-features=Vulkan','--ignore-gpu-blocklist','--enable-unsafe-webgpu','--disable-software-rasterizer']:[];
 if(config==='intel-coherent')env.VK_DRIVER_FILES='/usr/share/vulkan/icd.d/intel_icd.json';
 if(config==='nvidia-vulkan')Object.assign(env,{__NV_PRIME_RENDER_OFFLOAD:'1',__GLX_VENDOR_LIBRARY_NAME:'nvidia',__VK_LAYER_NV_optimus:'NVIDIA_only',VK_DRIVER_FILES:'/usr/share/vulkan/icd.d/nvidia_icd.json'});
 return{headless:true,executablePath:process.env.BENCH_CHROME||'/home/fidika/cozy_v2/outputs/canvas-library-selection/browser/chrome-linux64/chrome',args:['--no-sandbox',...extra],env,ignoreDefaultArgs:['--enable-unsafe-swiftshader']};
}
