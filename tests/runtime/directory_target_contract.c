#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#if defined(TEST_GR4) || defined(GRKIT_TARGET_GR4_121)
#define RT_FS_CLOSE 0x53ac3d80
#define ROOT 0x5537fc08U
#define FS_TABLE 0x53fd39f0U
#define FAT_TABLE 0x53fd3654U
#define EXFAT_TABLE 0x53fd06a4U
#define PRIMARY_TABLE 0x53fd0600U
#else
#define RT_FS_CLOSE 0x53845930
#define ROOT 0x55349e1cU
#define FS_TABLE 0x53d2d200U
#define FAT_TABLE 0x53d2ce64U
#define EXFAT_TABLE 0x53d29f4cU
#define PRIMARY_TABLE 0x53d29ea8U
#endif
#define FS 0x59000000U
#define DRIVES 0x59000100U
#define INSTANCE 0x59000204U
#define HANDLE 0x59000300U
#define RAW 0x59000400U
static u32 fs,drives,instance,raw,drive,state,table,primary,volume;
static int returned,calls,change,nonempty;
static u32 next_state;
static u32 word(u32 address)
{
    if(address==ROOT) return fs;
    if(address==FS) return FS_TABLE;
    if(address==FS+0x18) return drives;
    if(address==DRIVES) return instance;
    if(address==INSTANCE) return table;
    if(address==INSTANCE-4) return primary;
    if(address==HANDLE) return raw;
    if(address==HANDLE+4) return drive;
    if(address==RAW+0x11c) return volume;
    if(address==RAW+0x140) return state;
    assert(!"Unexpected memory access"); return 0;
}
static int native_read(u32 handle,void *out)
{
    assert(handle==HANDLE); ++calls;
    memset(out,0,0x330); if(nonempty) ((u8 *)out)[0]=1;
    state=next_state;
    if(change==1) fs=0;
    if(change==2) drives=0;
    if(change==3) instance=0;
    if(change==4) raw=0;
    if(change==5) drive=1;
    if(change==6) table=FAT_TABLE;
    if(change==7) primary=0;
    if(change==8) volume=1;
    return returned;
}
#define LIBRARY_DIRECTORY_WORD word
#define LIBRARY_DIRECTORY_NATIVE_READ native_read
#include "library_directory_target.h"
static void reset(void)
{
    fs=FS; drives=DRIVES; instance=INSTANCE; raw=RAW; drive=0;
    state=next_state=0; table=EXFAT_TABLE; primary=PRIMARY_TABLE; volume=0;
    calls=change=nonempty=returned=0;
}
int main(void)
{
    u8 out[2048];
    reset(); assert(library_directory_target_read(HANDLE,out)==0);
    reset(); next_state=0xfffffffeU; assert(library_directory_target_read(HANDLE,out)==0);
    state=0xfffffffeU; returned=-22; assert(library_directory_target_read(HANDLE,out)==1);
    for(int error=-1;error>=-16;--error) {
        if(error==-2) continue;
        reset(); state=(u32)error; returned=-22;
        assert(library_directory_target_read(HANDLE,out)==-5 && !calls);
        reset(); next_state=(u32)error;
        assert(library_directory_target_read(HANDLE,out)==-5);
    }
    reset(); returned=-22; assert(library_directory_target_read(HANDLE,out)==-5);
    reset(); state=next_state=0xfffffffeU; returned=-22; nonempty=1;
    assert(library_directory_target_read(HANDLE,out)==-5);
    reset(); state=next_state=0xfffffffeU; returned=0;
    assert(library_directory_target_read(HANDLE,out)==-5);
    for(int i=1;i<=8;++i) {
        reset(); state=next_state=0xfffffffeU; returned=-22; change=i;
        assert(library_directory_target_read(HANDLE,out)==-5);
    }
    reset(); drive=1; assert(library_directory_target_read(HANDLE,out)==-5 && !calls);
    reset(); raw=0; assert(library_directory_target_read(HANDLE,out)==-5 && !calls);
    reset(); assert(library_directory_target_read(0,out)==-5 && !calls);
    reset(); table=FAT_TABLE; returned=1;
    assert(library_directory_target_read(HANDLE,out)==1);
    reset(); table=FAT_TABLE; returned=-22;
    assert(library_directory_target_read(HANDLE,out)==-22);
    return 0;
}
