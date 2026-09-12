/*
 * fxc-shim: a minimal drop-in replacement for the Windows SDK `fxc.exe` that
 * gpui 0.2's release build script invokes to compile its HLSL shaders.
 *
 * The machine this project is developed on has no Windows SDK, hence no
 * fxc.exe, so `cargo build --release` panics in gpui's build.rs. This shim
 * speaks the subset of the fxc CLI that build.rs uses
 * (`/T <target> /E <entry> /Fh <out.h> /Vn <var> /O3 <shader.hlsl>`) and
 * compiles the shader through the system `d3dcompiler_47.dll`
 * (`D3DCompileFromFile` with `D3D_COMPILE_STANDARD_FILE_INCLUDE`), writing a
 * `const BYTE <var>[] = { ... };` header in the same shape fxc produces.
 *
 * Build (MinGW-w64 gcc, on PATH):
 *   gcc -O2 -o fxc.exe tools/fxc-shim/fxc.c -lkernel32
 *
 * Then point gpui at it before a release build:
 *   GPUI_FXC_PATH=<abs path to fxc.exe> cargo build --release -p navidog-app
 */

#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct ID3DBlob ID3DBlob;
typedef struct ID3DBlobVtbl {
    HRESULT (WINAPI *QueryInterface)(ID3DBlob *, const void *, void **);
    ULONG (WINAPI *AddRef)(ID3DBlob *);
    ULONG (WINAPI *Release)(ID3DBlob *);
    LPVOID (WINAPI *GetBufferPointer)(ID3DBlob *);
    SIZE_T (WINAPI *GetBufferSize)(ID3DBlob *);
} ID3DBlobVtbl;
struct ID3DBlob { ID3DBlobVtbl *lpVtbl; };

typedef HRESULT (WINAPI *D3DCompileFromFile_t)(
    LPCWSTR, const void *, void *, LPCSTR, LPCSTR, UINT, UINT,
    ID3DBlob **, ID3DBlob **);

static void to_wide(const char *src, wchar_t *dst, size_t n) {
    MultiByteToWideChar(CP_UTF8, 0, src, -1, dst, (int)n);
}

int main(int argc, char **argv) {
    const char *target = NULL, *entry = NULL, *outh = NULL;
    const char *varname = "ShaderBytes", *src = NULL;

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "/T") == 0 && i + 1 < argc) target = argv[++i];
        else if (strcmp(argv[i], "/E") == 0 && i + 1 < argc) entry = argv[++i];
        else if (strcmp(argv[i], "/Fh") == 0 && i + 1 < argc) outh = argv[++i];
        else if (strcmp(argv[i], "/Vn") == 0 && i + 1 < argc) varname = argv[++i];
        else if (argv[i][0] == '/') { /* ignore /O3 and other flags */ }
        else src = argv[i];
    }

    if (!target || !entry || !outh || !src) {
        fprintf(stderr, "fxc-shim: missing arguments\n");
        return 2;
    }

    HMODULE dll = LoadLibraryA("d3dcompiler_47.dll");
    if (!dll) {
        fprintf(stderr, "fxc-shim: cannot load d3dcompiler_47.dll\n");
        return 3;
    }
    D3DCompileFromFile_t compile =
        (D3DCompileFromFile_t)GetProcAddress(dll, "D3DCompileFromFile");
    if (!compile) {
        fprintf(stderr, "fxc-shim: D3DCompileFromFile not found\n");
        return 4;
    }

    wchar_t wsrc[MAX_PATH];
    to_wide(src, wsrc, MAX_PATH);

    ID3DBlob *code = NULL, *err = NULL;
    /* 0x8000 == D3DCOMPILE_OPTIMIZATION_LEVEL3; (void *)1 == D3D_COMPILE_STANDARD_FILE_INCLUDE */
    HRESULT hr = compile(wsrc, NULL, (void *)1, entry, target, 0x8000u, 0, &code, &err);
    if (FAILED(hr)) {
        if (err) {
            fprintf(stderr, "%.*s\n",
                    (int)err->lpVtbl->GetBufferSize(err),
                    (char *)err->lpVtbl->GetBufferPointer(err));
        } else {
            fprintf(stderr, "fxc-shim: compile failed 0x%08lx\n", (unsigned long)hr);
        }
        return 1;
    }

    FILE *f = fopen(outh, "wb");
    if (!f) {
        fprintf(stderr, "fxc-shim: cannot open %s\n", outh);
        return 5;
    }

    const unsigned char *bytes =
        (const unsigned char *)code->lpVtbl->GetBufferPointer(code);
    SIZE_T size = code->lpVtbl->GetBufferSize(code);

    fprintf(f, "const BYTE %s[] =\n{\n", varname);
    for (SIZE_T i = 0; i < size; i++) {
        fprintf(f, "0x%02x,", bytes[i]);
        if ((i & 15) == 15) fputc('\n', f);
    }
    fprintf(f, "\n};\n");
    fclose(f);
    return 0;
}
