@echo off
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
set CMAKE_GENERATOR=Ninja
set CMAKE_MAKE_PROGRAM=C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe
set CMAKE_GENERATOR_INSTANCE=
set CMAKE_GENERATOR_TOOLSET=
set CMAKE_GENERATOR_PLATFORM=
set CUDACXX=C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.4\bin\nvcc.exe
set CUDA_PATH=C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.4
set PATH=C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.4\bin;%PATH%
cd /d C:\Users\Niketas\projects\dictator\apps\windows
cargo build --release --features cuda
