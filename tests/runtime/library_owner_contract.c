#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint32_t u32;
typedef uint8_t u8;
#define LIBRARY_OWNER_BARRIER() __atomic_thread_fence(__ATOMIC_SEQ_CST)
#include "library_owner.h"

static u8 files[2][LIBRARY_STORE_SLOT_BYTES];
static u32 position[2],opens,closes,reads,writes,flushes;
static int active[2],resident,short_read,short_write,close_failure,wrong_size;
static int fs_open(const char *path,u32 mode,u32 amode)
{
    assert(resident); assert(mode==0 && amode==0);
    u32 slot=!strcmp(path,"C:\\GRKIT\\LIB1.BIN");
    assert(!strcmp(path,library_owner_path(slot)));
    assert(!active[slot]); active[slot]=1; position[slot]=0; ++opens;
    return (int)slot; /* Descriptor zero is valid. */
}
static int fs_close(u32 handle)
{
    assert(resident && handle<2 && active[handle]); active[handle]=0; ++closes;
    if(close_failure) { close_failure=0; return 1; }
    return 0;
}
static int fs_read(u32 handle,void *out,u32 count,u32 *length)
{
    assert(resident && handle<2 && active[handle]);
    assert(count<=LIBRARY_STORE_SLOT_BYTES-position[handle]); ++reads;
    u32 amount=short_read && count ? count-1 : count;
    memcpy(out,files[handle]+position[handle],amount); position[handle]+=amount; *length=amount;
    return 0;
}
static int fs_write(u32 handle,void *in,u32 count,u32 *length)
{
    assert(resident && handle<2 && active[handle]);
    assert(count<=LIBRARY_STORE_SLOT_BYTES-position[handle]); ++writes;
    u32 amount=short_write && count ? count-1 : count;
    memcpy(files[handle]+position[handle],in,amount); position[handle]+=amount; *length=amount;
    return 0;
}
static int fs_stat(const char *path,void *out)
{
    assert(resident); assert(!strcmp(path,library_owner_path(0)) || !strcmp(path,library_owner_path(1)));
    ((u32 *)out)[2]=wrong_size ? 0 : LIBRARY_STORE_SLOT_BYTES;
    return 0;
}
static int durable(void *context)
{ (void)context; assert(resident && !active[0] && !active[1]); ++flushes; return 0; }
static int deny_write(void *context) { (void)context; return 1; }
static int is_resident(void *context) { (void)context; return resident; }
static int contents_ready(void *context,const u8 *bytes,u32 count)
{ (void)context; return !library_store_manifest(bytes,count); }
#define RT_FS_OPEN fs_open
#define RT_FS_CLOSE fs_close
#define RT_FS_READ fs_read
#define RT_FS_WRITE fs_write
#define RT_FS_STAT_SIZE fs_stat
#include "library_owner_target.h"

static struct library_owner owner;
static u8 empty[32];
static void init(void)
{
    memset(files,0,sizeof(files)); memset(active,0,sizeof(active));
    opens=closes=reads=writes=flushes=0; resident=0;
    short_read=short_write=close_failure=wrong_size=0;
    struct library_owner_fs fs=library_owner_target_fs();
    assert(!fs.durable);
    assert(library_owner_init(&owner,&fs,0,is_resident,contents_ready));
    memset(empty,0,sizeof(empty)); memcpy(empty,"GRKLIB01",8);
    library_store_put(empty+8,1); library_store_put(empty+12,32);
    library_store_put(empty+24,library_store_crc(empty,32,24,4));
}
static void take(int expected,u32 generation)
{
    int status=-1; struct library_store_view view;
    assert(library_owner_result(&owner,&status,&view));
    assert(status==expected && view.generation==generation);
    assert(!library_owner_result(&owner,&status,&view));
    assert(!active[0] && !active[1] && opens==closes);
}
static void scheduling_and_durability_gate(void)
{
    init();
    assert(library_owner_submit(&owner,LIBRARY_OWNER_SEED,0,empty,32));
    assert(!library_owner_submit(&owner,LIBRARY_OWNER_SEED,0,empty,32));
    assert(!opens && !writes);
    assert(library_owner_poll(&owner)==LIBRARY_OWNER_NOT_RESIDENT);
    assert(owner.state==LIBRARY_OWNER_QUEUED && !opens && !writes);
    resident=1;
    assert(library_owner_poll(&owner)==LIBRARY_OWNER_UNVERIFIED_DURABILITY);
    take(LIBRARY_OWNER_UNVERIFIED_DURABILITY,0);
    assert(!opens && !writes);
    owner.fs.durable=durable; /* A verified barrier in this host filesystem. */
    owner.fs.admit_write=deny_write;
    assert(library_owner_submit(&owner,LIBRARY_OWNER_SEED,0,empty,32));
    assert(library_owner_poll(&owner)==LIBRARY_OWNER_UNVERIFIED_DURABILITY);
    take(LIBRARY_OWNER_UNVERIFIED_DURABILITY,0);
    assert(!opens && !writes && !flushes);
    owner.fs.admit_write=0;
    assert(library_owner_submit(&owner,LIBRARY_OWNER_SEED,0,empty,32));
    empty[0]='X'; /* Submit copied the candidate; producer lifetime is separate. */
    assert(library_owner_poll(&owner)==LIBRARY_STORE_OK);
    empty[0]='G';
    take(LIBRARY_STORE_OK,1);
    assert(flushes==3 && !memcmp(owner.scratch,empty,32));
    owner.fs.durable=0;
    assert(library_owner_submit(&owner,LIBRARY_OWNER_RECOVER,0,0,0));
    assert(library_owner_poll(&owner)==LIBRARY_STORE_OK);
    take(LIBRARY_STORE_OK,1);
    owner.fs.durable=durable;
    assert(library_owner_submit(&owner,LIBRARY_OWNER_COMMIT,1,empty,32));
    assert(library_owner_poll(&owner)==LIBRARY_STORE_OK);
    take(LIBRARY_STORE_OK,2);
    assert(flushes==6);
}
static void offset_io_and_errors(void)
{
    init(); resident=1;
    const u8 data[4]={1,2,3,4}; u8 readback[4]={0};
    assert(!library_owner_write(&owner,0,32760,data,4));
    assert(!memcmp(files[0]+32760,data,4));
    assert(!library_owner_read(&owner,0,32760,readback,4));
    assert(!memcmp(data,readback,4));
    assert(!library_owner_close_all(&owner));
    assert(!active[0] && !active[1] && opens==closes);
    u32 before=writes;
    assert(library_owner_write(&owner,0,32767,data,4));
    assert(writes==before);
    short_read=1;
    assert(library_owner_read(&owner,0,100,readback,4));
    assert(!active[0]); short_read=0;
    short_write=1;
    assert(library_owner_write(&owner,0,100,data,4));
    assert(!active[0]); short_write=0;
    wrong_size=1;
    assert(library_owner_read(&owner,0,0,readback,4));
    assert(!active[0]); wrong_size=0;
    close_failure=1;
    assert(!library_owner_read(&owner,0,0,readback,4));
    assert(library_owner_close_all(&owner));
    assert(!active[0] && opens==closes);
}
static void failed_recovery_does_not_publish_a_generation(void)
{
    init(); resident=1; owner.fs.durable=durable;
    assert(library_owner_submit(&owner,LIBRARY_OWNER_SEED,0,empty,32));
    assert(!library_owner_poll(&owner));
    take(LIBRARY_STORE_OK,1);
    /* The loader retains the current catalog until a complete result exists.
       A file error must not publish an uninitialized/new generation. */
    short_read=1;
    assert(library_owner_submit(&owner,LIBRARY_OWNER_RECOVER,0,0,0));
    assert(library_owner_poll(&owner)==LIBRARY_STORE_IO);
    take(LIBRARY_STORE_IO,1);
    short_read=0;
    memset(files,0,sizeof(files));
    assert(library_owner_submit(&owner,LIBRARY_OWNER_RECOVER,0,0,0));
    assert(library_owner_poll(&owner)==LIBRARY_STORE_RECOVERY_REQUIRED);
    take(LIBRARY_STORE_RECOVERY_REQUIRED,1);
    /* Failure does not silently seed a replacement library. */
    assert(flushes==3);
}
int main(void)
{
    scheduling_and_durability_gate();
    offset_io_and_errors();
    failed_recovery_does_not_publish_a_generation();
    return 0;
}
