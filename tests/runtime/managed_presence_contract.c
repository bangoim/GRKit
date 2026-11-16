#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
static int parent_present,slot_present,open_error,read_error,close_error,malformed,endless;
static u32 row;
static int open_dir(const char *name,u32 *handle)
{ row=0; *handle=!strcmp(name,"C:\\")?1:2; return open_error; }
static int read_dir(u32 handle,void *out)
{
    if(read_error) return 2;
    if(malformed) { memset(out,'x',768); return 0; }
    if(endless) { strcpy(out,"OTHER"); return 0; }
    if(row++) return 1;
    if(handle==1 && parent_present) { strcpy(out,"grkit"); return 0; }
    if(handle==2 && slot_present) { strcpy(out,"lib0.bin"); return 0; }
    return 1;
}
static int close_dir(u32 handle) { (void)handle; return close_error; }
#define FS_OPENDIR open_dir
#define FS_READDIR read_dir
#define FS_CLOSEDIR close_dir
#include "fastboot/managed_presence.h"
int main(void)
{
    u8 scratch[2048];
    assert(fast_managed_presence("LIB0.BIN",scratch)==0);
    parent_present=1; assert(fast_managed_presence("LIB0.BIN",scratch)==0);
    slot_present=1; assert(fast_managed_presence("LIB0.BIN",scratch)==1);
    assert(fast_managed_presence("LIB1.BIN",scratch)==0);
    assert(fast_managed_presence("LIB.ACT",scratch)==0);
    open_error=1; assert(fast_managed_presence("LIB0.BIN",scratch)==-1); open_error=0;
    read_error=1; assert(fast_managed_presence("LIB0.BIN",scratch)==-1); read_error=0;
    close_error=1; assert(fast_managed_presence("LIB0.BIN",scratch)==-1); close_error=0;
    malformed=1; assert(fast_managed_presence("LIB0.BIN",scratch)==-1); malformed=0;
    endless=1; assert(fast_managed_presence("LIB0.BIN",scratch)==-1);
    return 0;
}
