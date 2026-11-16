#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#define LIBRARY_OWNER_BARRIER() __atomic_thread_fence(__ATOMIC_SEQ_CST)
#include "library_objects.h"
#include "grf.h"
static u8 extended[GRKIT_GRF_MAX_SIZE],extended_hash[32];
static u8 grf[GRKIT_GRF_SIZE],file[GRKIT_GRF_MAX_SIZE],scratch[GRKIT_GRF_MAX_SIZE],hash[32];
static char path[32];
static u32 size,created,written,position,cursor,reads,closes;
static int present,active,resident=1,fail_read,fail_write,fail_sync,fail_listing,fail_close,fail_stat,fail_read_at,corrupt_write,fail_close_at;
static int validate(const u8 *p,u32 n) { return grkit_grf_validate(p,n); }
static int task(void *ctx) { (void)ctx; return resident; }
static int admit(void *ctx) { (void)ctx; return 0; }
static int durable(void *ctx) { (void)ctx; assert(!active); return fail_sync; }
static int open_file(const char *name,u32 mode,u32 amode) { assert(!strcmp(name,path) && !mode && !amode && present && !active); active=1; cursor=0; return 0; }
static int close_file(u32 handle) { assert(!handle && active); active=0; ++closes; return fail_close || (fail_close_at && closes==(u32)fail_close_at); }
static int read_file(u32 handle,void *out,u32 n,u32 *count) { assert(!handle && active && !cursor && n==size); ++reads; if(fail_read || (fail_read_at && reads==(u32)fail_read_at)) return 1; memcpy(out,file,n); *count=n; cursor=n; return 0; }
static int write_file(u32 handle,void *in,u32 n,u32 *count) { assert(!handle && active && !cursor && n>=size); ++written; if(fail_write) n/=2; memcpy(file,in,n); if(corrupt_write && n) file[n-1]^=1; if(n>size) size=n; *count=n; cursor=n; return 0; }
static int stat_file(const char *name,void *out) { assert(!strcmp(name,path)); if(!present || fail_stat) return 1; ((u32 *)out)[2]=size; return 0; }
static int create_file(const char *name,u32 mode) { assert(!strcmp(name,path) && !mode && !present); ++created; present=active=1; cursor=0; return 0; }
static int open_dir(const char *name,u32 *handle) { assert(!strcmp(name,"C:\\GRKIT\\CONTENT")); *handle=1; position=0; return 0; }
static int read_dir(u32 handle,void *out) { assert(handle==1); if(fail_listing) return 2; if(position++ || !present) return 1; strcpy(out,path+17); return 0; }
static int close_dir(u32 handle) { assert(handle==1); return 0; }
int main(void)
{
    memcpy(grf,"GRFX0001",8); library_store_put(grf+GRKIT_GRF_BYTES,sizeof(grf));
    memcpy(grf+GRKIT_GRF_ID,"sample",6); memcpy(grf+GRKIT_GRF_NAME,"Sample",6);
    library_store_put(grf+GRKIT_GRF_CRC,library_store_crc(grf,sizeof(grf),GRKIT_GRF_CRC,4));
    assert(validate(grf,sizeof(grf))); assert(library_sha256(grf,sizeof(grf),hash));
    struct library_manifest_entry entry={grf+GRKIT_GRF_ID,grf+GRKIT_GRF_NAME,hash,9,0};
    struct library_owner owner={0}; owner.fs=(struct library_owner_fs){open_file,close_file,read_file,write_file,stat_file,durable,admit}; owner.is_resident=task;
    struct library_objects objects={&owner,create_file,open_dir,read_dir,close_dir,scratch,sizeof(scratch)};
    library_object_path(hash,path);
    resident=0; assert(library_object_install(&objects,&entry,grf,sizeof(grf),validate)==LIBRARY_OWNER_NOT_RESIDENT); assert(!created); resident=1;
    fail_listing=1; assert(library_object_install(&objects,&entry,grf,sizeof(grf),validate)==LIBRARY_STORE_IO); assert(!created); fail_listing=0;
    assert(library_object_install(&objects,&entry,grf,sizeof(grf),validate)==0); assert(created==1 && written==1 && !active);
    assert(library_object_install(&objects,&entry,grf,sizeof(grf),validate)==0); assert(created==1 && written==1);
    file[GRKIT_GRF_ICON]^=1;
    assert(library_object_install(&objects,&entry,grf,sizeof(grf),validate)==LIBRARY_OBJECT_CONFLICT); assert(created==1 && written==1);
    present=0; size=0; fail_write=1;
    assert(library_object_install(&objects,&entry,grf,sizeof(grf),validate)==LIBRARY_STORE_IO); assert(!active); fail_write=0;
    present=0; size=0; fail_sync=1;
    assert(library_object_install(&objects,&entry,grf,sizeof(grf),validate)==LIBRARY_STORE_IO); assert(!active); fail_sync=0;
    fail_read=1; assert(library_object_install(&objects,&entry,grf,sizeof(grf),validate)==LIBRARY_STORE_IO); assert(!active); fail_read=0;
    /* Failed creation leaves a short orphan; only an authorized matching
       prefix can recover it, without creating/replacing its directory entry. */
    size=117; memcpy(file,grf,size); u32 writes=written,creates=created;
    assert(library_object_install(&objects,&entry,grf,sizeof(grf),validate)==LIBRARY_OBJECT_CONFLICT);
    assert(written==writes);
    fail_stat=1;
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_STORE_IO);
    fail_stat=0; assert(written==writes);
    fail_read=1;
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_STORE_IO);
    fail_read=0; assert(written==writes && !active);
    fail_close=1;
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_STORE_IO);
    fail_close=0; assert(written==writes && !active);
    file[50]^=1;
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_OBJECT_CONFLICT);
    assert(written==writes); file[50]^=1;
    assert(!library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1));
    assert(created==creates && written==writes+1 && size==sizeof(grf) && !memcmp(file,grf,size));
    assert(!library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1));
    assert(written==writes+1);
    /* Full invalid or valid-but-different objects are never rebuilt. */
    file[GRKIT_GRF_ICON]^=1;
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_OBJECT_CONFLICT);
    assert(written==writes+1);
    library_store_put(file+GRKIT_GRF_CRC,library_store_crc(file,size,GRKIT_GRF_CRC,4));
    assert(validate(file,size));
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_OBJECT_CONFLICT);
    assert(written==writes+1);
    size=0;
    assert(!library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1));
    assert(size==sizeof(grf) && created==creates);
    size=100; memcpy(file,grf,size); fail_write=1;
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_STORE_IO);
    assert(!active && size==sizeof(grf)/2); fail_write=0;
    assert(!library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1));
    size=100; closes=0; fail_close_at=2;
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_STORE_IO);
    fail_close_at=0; assert(!active && size==sizeof(grf));
    size=100; fail_sync=1;
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_STORE_IO);
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_STORE_IO);
    fail_sync=0; assert(!active);
    writes=written;
    assert(!library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1));
    assert(written==writes);
    size=100; reads=0; fail_read_at=2;
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_STORE_IO);
    fail_read_at=0; assert(!active);
    size=100; corrupt_write=1;
    assert(library_object_install_repair(&objects,&entry,grf,sizeof(grf),validate,1)==LIBRARY_OBJECT_CONFLICT);
    corrupt_write=0; assert(!active);
    /* 8192 is repairable only if it is an incomplete 8704-byte GRF, not
       another valid short-form object with the same 8-hex path prefix. */
    memcpy(extended,grf,sizeof(grf)); library_store_put(extended+GRKIT_GRF_BYTES,sizeof(extended));
    library_store_put(extended+GRKIT_GRF_FEATURES,GRKIT_FEATURE_FN_DEFAULTS);
    library_store_put(extended+GRKIT_GRF_CRC,library_store_crc(extended,sizeof(extended),GRKIT_GRF_CRC,4));
    assert(validate(extended,sizeof(extended))); assert(library_sha256(extended,sizeof(extended),extended_hash));
    struct library_manifest_entry long_entry={extended+GRKIT_GRF_ID,extended+GRKIT_GRF_NAME,extended_hash,9,GRKIT_FEATURE_FN_DEFAULTS};
    library_object_path(extended_hash,path); size=sizeof(grf); memcpy(file,grf,size); writes=written;
    assert(library_object_install_repair(&objects,&long_entry,extended,sizeof(extended),validate,1)==LIBRARY_OBJECT_CONFLICT);
    assert(written==writes);
    memcpy(file,extended,size);
    assert(!library_object_install_repair(&objects,&long_entry,extended,sizeof(extended),validate,1));
    assert(size==sizeof(extended) && !memcmp(file,extended,size) && created==creates);
    return 0;
}
