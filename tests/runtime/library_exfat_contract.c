#include <assert.h>
#include <stdint.h>
typedef uint32_t u32;
#ifndef RT_FS_CLOSE
#define RT_FS_CLOSE 0x53ac3d80
#endif
#if RT_FS_CLOSE == 0x53845930
#define TEST_FS_ROOT 0x55349e1cU
#define TEST_FS_VTABLE 0x53d2d200U
#define TEST_PRIMARY_VTABLE 0x53d29ea8U
#define TEST_SECONDARY_VTABLE 0x53d29f4cU
#define TEST_MANAGER 0x55349decU
#define TEST_VOLUME_TABLE 0x5537a238U
#define TEST_STRIDE 0x18U
#define TEST_MUTEX 0x59000218U
#else
#define TEST_FS_ROOT 0x5537fc08U
#define TEST_FS_VTABLE 0x53fd39f0U
#define TEST_PRIMARY_VTABLE 0x53fd0600U
#define TEST_SECONDARY_VTABLE 0x53fd06a4U
#define TEST_MANAGER 0x5537fbd8U
#define TEST_VOLUME_TABLE 0x553af308U
#define TEST_STRIDE 0x1cU
#define TEST_MUTEX 0x5900021cU
#endif
static u32 c_index,count,state,type,instance,locks,flushes,unlocks;
static int duplicate,lock_failure,flush_failure,unlock_failure,change_on_lock,change_on_flush;
static u32 word(u32 address)
{
    switch(address) {
    case TEST_FS_ROOT: return 0x59000000U;
    case 0x59000000U: return TEST_FS_VTABLE;
    case 0x59000018U: return 0x59000100U;
    case 0x59000100U: return instance;
    case 0x59000200U: return TEST_PRIMARY_VTABLE;
    case 0x59000204U: return TEST_SECONDARY_VTABLE;
    case TEST_MANAGER: return 0x59000300U;
    case 0x59000314U: return 0x59000400U;
    case 0x59000320U: return count;
    case TEST_VOLUME_TABLE: case TEST_VOLUME_TABLE+0x50U: return state;
    case TEST_VOLUME_TABLE+0x20U: case TEST_VOLUME_TABLE+0x70U: return type;
    default:
        assert(address>=0x59000404U && address<0x59000404U+26U*TEST_STRIDE);
        assert((address-0x59000404U)%TEST_STRIDE==0);
        return (address-0x59000404U)/TEST_STRIDE==c_index || duplicate ? 'C' : 'I';
    }
}
static u32 lock(u32 mutex) {
    assert(mutex==TEST_MUTEX); ++locks;
    if(change_on_lock) instance=0;
    return !lock_failure;
}
static int flush(u32 volume) {
    assert(volume==c_index); ++flushes;
    if(change_on_flush) state=0;
    return flush_failure?-10009:0;
}
static u32 unlock(u32 mutex) {
    assert(mutex==TEST_MUTEX); ++unlocks; return !unlock_failure;
}
#define LIBRARY_EXFAT_WORD(address) word(address)
#define LIBRARY_EXFAT_LOCK(mutex) lock(mutex)
#define LIBRARY_EXFAT_UNLOCK(mutex) unlock(mutex)
#define LIBRARY_EXFAT_FLUSH(volume) flush(volume)
#include "library_exfat_target.h"
static void reset(void) {
    c_index=0; count=3; state=0x101U; type=0; instance=0x59000204U;
    locks=flushes=unlocks=0;
    duplicate=lock_failure=flush_failure=unlock_failure=change_on_lock=change_on_flush=0;
}
int main(void) {
    reset(); assert(library_exfat_target_probe()); assert(!library_exfat_target_flush());
    assert(locks==1 && flushes==1 && unlocks==1);
    reset(); c_index=1; state=0x10101U; assert(!library_exfat_target_flush());
    reset(); state=0; assert(!library_exfat_target_probe());
    reset(); state=0x201U; assert(!library_exfat_target_probe());
    reset(); state=0x10101U; assert(!library_exfat_target_probe());
    reset(); type=1; assert(!library_exfat_target_probe());
    reset(); duplicate=1; assert(!library_exfat_target_probe());
    reset(); c_index=2; assert(!library_exfat_target_probe());
    reset(); count=0; assert(library_exfat_target_flush()); assert(!locks);
    reset(); count=27; assert(!library_exfat_target_probe());
    reset(); instance=0x6fffffe4U; assert(!library_exfat_target_probe());
    reset(); instance=0x6ffffffcU; assert(!library_exfat_target_probe());
    reset(); lock_failure=1; assert(library_exfat_target_flush()); assert(!flushes && !unlocks);
    reset(); change_on_lock=1; assert(library_exfat_target_flush()); assert(!flushes && unlocks==1);
    reset(); flush_failure=1; assert(library_exfat_target_flush()); assert(flushes==1 && unlocks==1);
    reset(); change_on_flush=1; assert(library_exfat_target_flush()); assert(unlocks==1);
    reset(); unlock_failure=1; assert(library_exfat_target_flush()); assert(unlocks==1);
    return 0;
}
