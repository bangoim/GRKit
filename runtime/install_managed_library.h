#ifndef GRKIT_INSTALL_MANAGED_LIBRARY_H
#define GRKIT_INSTALL_MANAGED_LIBRARY_H
#include "library_content.h"
#include "library_activation.h"
/* Activation makes a managed catalog authoritative. Before activation, only
   proven initial seed/preallocation state permits the legacy path. */
static u32 managed_library;
static long managed_slots[2]={-2,-2};
static u8 managed_manifest_bytes[LIBRARY_STORE_MANIFEST_MAX];
static struct library_manifest managed_manifest;
static u8 managed_discard[256];
static const char *managed_path(u32 slot)
{ return slot ? DATA_PATH "/LIB1.BIN" : DATA_PATH "/LIB0.BIN"; }
static int managed_close(void)
{
    int failed=0;
    for(u32 i=0;i<2;++i) if(managed_slots[i]>=0) {
        if(syscall6(6,managed_slots[i],0,0,0,0,0)<0) failed=1;
        managed_slots[i]=-2;
    }
    return failed;
}
static int managed_read(void *owner,u32 slot,u32 offset,u8 *out,u32 size)
{
    (void)owner;
    if(slot>1 || offset>LIBRARY_STORE_SLOT_BYTES || size>LIBRARY_STORE_SLOT_BYTES-offset) return 1;
    /* An absent slot is an invalid generation, not an unreadable generation. */
    if(managed_slots[slot]==-2) { zero(out,size); return 0; }
    if(managed_slots[slot]<0) return 1;
    if(syscall6(6,managed_slots[slot],0,0,0,0,0)<0) return 1;
    managed_slots[slot]=syscall6(5,(u32)managed_path(slot),0x8800,0,0,0,0);
    if(managed_slots[slot]<0) { managed_slots[slot]=-5; return 1; }
    /* The native file backend has no seek operation. Reopening also avoids
       coupling this read-only path to Linux file-position assumptions. */
    for(u32 done=0;done<offset;) {
        u32 take=offset-done; if(take>sizeof(managed_discard)) take=sizeof(managed_discard);
        long n=syscall6(3,managed_slots[slot],(u32)managed_discard,take,0,0,0);
        if(n<=0 || (u32)n>take) return 1;
        done+=(u32)n;
    }
    for(u32 done=0;done<size;) {
        long n=syscall6(3,managed_slots[slot],(u32)(out+done),size-done,0,0,0);
        if(n<=0 || (u32)n>size-done) return 1;
        done+=(u32)n;
    }
    if(offset+size==LIBRARY_STORE_SLOT_BYTES &&
       syscall6(3,managed_slots[slot],(u32)managed_discard,1,0,0,0)!=0) return 1;
    return 0;
}
static int managed_no_write(void *owner,u32 slot,u32 offset,const u8 *bytes,u32 size)
{ (void)owner;(void)slot;(void)offset;(void)bytes;(void)size; return 1; }
static int managed_no_sync(void *owner,u32 slot)
{ (void)owner;(void)slot; return 1; }
static void managed_filename(const u8 hash[32],char name[13])
{
    static const char hex[]="0123456789ABCDEF";
    for(u32 i=0;i<4;++i) { name[2*i]=hex[hash[i]>>4]; name[2*i+1]=hex[hash[i]&15]; }
    copy((u8 *)name+8,(const u8 *)".GRF",5);
}
static u32 managed_content(const struct library_manifest_entry *entry,u8 *bytes)
{
    char path[sizeof(DATA_PATH)+32],name[13];
    managed_filename(entry->hash,name);
    if(!join(path,sizeof(path),DATA_PATH "/CONTENT",name)) return 0;
    u32 length=read_effect(path,bytes);
    return length && library_content_matches(entry,bytes,length,valid_profile) ? length : 0;
}
static int managed_contents(void *owner,const u8 *bytes,u32 length)
{
    (void)owner;
    if(!library_manifest_open(bytes,length,&managed_manifest)) return 1;
    for(u32 i=0;i<managed_manifest.entries;++i) {
        struct library_manifest_entry entry; library_manifest_get(&managed_manifest,i,&entry);
        if(!(entry.flags&2U) && !managed_content(&entry,temporary)) return 1;
    }
    return 0;
}
/* No activation means seed/preallocation may have been interrupted. Read
   every byte available without requiring a committed slot, but never accept
   an I/O error or evidence that ordinary (generation >= 2) writes began. */
static int managed_unactivated_slot(u32 slot)
{
    long fd=managed_slots[slot];
    if(fd==-2) return 0;
    if(fd<0) return 1;
    u8 header[32]={0},commit[16]={0}; u32 offset=0;
    for(;;) {
        long n=syscall6(3,fd,(u32)managed_discard,sizeof(managed_discard),0,0,0);
        if(n<0 || n>(long)sizeof(managed_discard)) return 1;
        if(!n) break;
        if((u32)n>LIBRARY_STORE_SLOT_BYTES-offset) return 1;
        for(u32 i=0;i<(u32)n;++i) {
            u32 at=offset+i;
            if(at<32) header[at]=managed_discard[i];
            if(at>=LIBRARY_STORE_COMMIT_OFFSET) commit[at-LIBRARY_STORE_COMMIT_OFFSET]=managed_discard[i];
        }
        offset+=(u32)n;
    }
    /* A torn magic/CRC must not hide a previously used generation. */
    return (offset>=12 && get32(header+8)>=2) ||
           (offset>=LIBRARY_STORE_COMMIT_OFFSET+8 && get32(commit+4)>=2);
}
static int managed_probe(void)
{
    managed_library=1;
    if(managed_close()) return STATUS_LIBRARY_UNREADABLE;
    u8 marker[LIBRARY_ACTIVATION_BYTES+1]; u32 used=0,seed_checksum=0;
    long fd=syscall6(5,(u32)(DATA_PATH "/LIB.ACT"),0x8800,0,0,0,0);
    int absent=fd==-2,failed=fd<0 && !absent;
    if(fd>=0) {
        while(used<sizeof(marker)) {
            long n=syscall6(3,fd,(u32)(marker+used),sizeof(marker)-used,0,0,0);
            if(n<0 || n>(long)(sizeof(marker)-used)) { failed=1; break; }
            if(!n) break;
            used+=(u32)n;
        }
        if(syscall6(6,fd,0,0,0,0,0)<0) failed=1;
        if(!library_activation_validate(marker,used,&seed_checksum)) failed=1;
    }
    managed_library=1;
    if(failed) return STATUS_LIBRARY_UNREADABLE;
    managed_slots[0]=syscall6(5,(u32)managed_path(0),0x8800,0,0,0,0);
    managed_slots[1]=syscall6(5,(u32)managed_path(1),0x8800,0,0,0,0);
    if(absent) {
        failed=managed_unactivated_slot(0) || managed_unactivated_slot(1);
        if(managed_close()) failed=1;
        if(failed) return STATUS_LIBRARY_UNREADABLE;
        managed_library=0;
        return 0;
    }
    const struct library_store_io io={0,managed_read,managed_no_write,managed_no_sync,managed_contents};
    struct library_store_view view;
    int result=library_store_recover(&io,managed_manifest_bytes,sizeof(managed_manifest_bytes),&view);
    if(!result && view.generation==1 && view.checksum!=seed_checksum) result=LIBRARY_STORE_RECOVERY_REQUIRED;
    if(managed_close()) result=LIBRARY_STORE_IO;
    return result ? STATUS_LIBRARY_UNREADABLE : 0;
}
static int managed_discover(void)
{
    count=0; listed_count=0; overflow_count=0;
#ifdef GRKIT_NO_IMAGE_CONTROL
    return managed_manifest.listed ? STATUS_LIBRARY_UNREADABLE : 0;
#endif
    if(managed_manifest.listed && !tone_used()) return STATUS_LIBRARY_UNREADABLE;
    listed_count=managed_manifest.listed;
    for(u32 pass=0;pass<2;++pass) for(u32 i=0;i<listed_count;++i) {
        struct library_manifest_entry entry;
        library_manifest_get(&managed_manifest,library_manifest_order(&managed_manifest,i),&entry);
        u32 off=!(entry.flags&1U);
        if(off!=pass) continue;
        if(!managed_content(&entry,temporary)) return STATUS_LIBRARY_UNREADABLE;
        copy(listed[i],temporary,LISTED_HEAD);
        managed_filename(entry.hash,names[i]);
        listed_choice[i]=((i+1)<<1)|off;
        listed_record[i]=EFFECTS_NO_RECORD;
        listed_status[i]=off ? EFFECT_OFF : EFFECT_OVER;
        if(count<MAX_COUNT) {
            copy(profiles[count],temporary,GRKIT_GRF_MAX_SIZE);
            listed_record[i]=count++;
            if(!off) listed_status[i]=EFFECT_LOADED;
        } else if(!off) return STATUS_LIBRARY_UNREADABLE;
    }
    return 0;
}
static void plan_modules(void);
/* The same branch is used by SCAN and the native restart contract. Probe
   precedes module/state preparation and every legacy effect import. */
static int managed_scan(void)
{
    int status=managed_probe();
    if(status) return status;
    plan_modules();
    if(managed_library) return managed_discover();
    collect();
    return discover();
}
/* State may restore a selected code only if the authoritative catalog enables
   that exact installed record. Deleted and disabled records never reactivate. */
static int managed_selected(u32 record)
{
    if(!managed_library) return 1;
    for(u32 i=0;i<listed_count;++i)
        if(listed_record[i]==record && !(listed_choice[i]&1U)) return 1;
    return 0;
}
#endif
