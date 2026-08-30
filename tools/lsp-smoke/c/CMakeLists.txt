cmake_minimum_required(VERSION 3.16)
project(bli_kvm_lsp_probe LANGUAGES C)

set(CMAKE_C_STANDARD 17)
set(CMAKE_C_STANDARD_REQUIRED ON)

add_library(bli_kvm_c_lsp_probe OBJECT probe.c)
