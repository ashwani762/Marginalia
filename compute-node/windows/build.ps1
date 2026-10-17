# Builds marginalia-node and marginalia-node-tray in release mode, picking the GPU backend by itself.
#   .\compute-node\windows\build.ps1             pick CUDA, Vulkan or CPU automatically
#   .\compute-node\windows\build.ps1 -Cpu        force the CPU only build
# The results are target\release\marginalia-node.exe and target\release\marginalia-node-tray.exe
# in the repository. The first build compiles llama.cpp and takes a long while.
param(
    [switch]$Cpu,
    [switch]$Vulkan,
    [switch]$Cuda
)
. (Join-Path $PSScriptRoot 'common.ps1')
Update-SessionPath

$ok = Invoke-MnBuild $Cpu.IsPresent $Vulkan.IsPresent $Cuda.IsPresent
if ($ok) {
    Step-Ok ('Done. The programs are in ' + (Join-Path $MN_RepoRoot 'target\release'))
    exit 0
}
exit 1
