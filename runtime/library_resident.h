#ifndef GRKIT_LIBRARY_RESIDENT_H
#define GRKIT_LIBRARY_RESIDENT_H
/* Executed only by the resident Script task through OWNER.BIN. This entry is
   linked with MENU for relocation, but no App callback performs file I/O. */
#if (defined(GRKIT_TARGET_GR3_221) || defined(GRKIT_TARGET_GR4_121)) && defined(__arm__)
#if defined(GRKIT_TARGET_GR3_221)
#include "fastboot/targets/gr3-2.21.h"
#else
#include "fastboot/targets/gr4-1.21.h"
#endif
#include "library_owner_target.h"
#include "library_content.h"
#include "library_objects.h"
#include "library_card_target.h"
#include "library_clock.h"
#include "library_snapshot_stream.h"
#include "library_telemetry_metadata.h"
#include "library_seed.h"
#include "library_activation.h"
#include "library_apply_stream.h"
#include "library_candidate.h"
#include "library_pending.h"
#include "catalog.h"
#include "grf.h"
struct library_resident {
    u32 magic,header,recovered,status;
    struct library_owner owner;
    u8 content[GRKIT_GRF_MAX_SIZE];
    u8 card_scratch[sizeof(struct library_card_scratch)+63];
    struct look_telemetry telemetry;
    struct library_manifest manifest;
    struct library_telemetry_metadata metadata_scratch;
    struct library_snapshot_metadata metadata;
    struct library_snapshot_stream snapshot;
    struct library_apply_stream apply;
    u8 card_evidence[532];
    u32 apply_action,apply_index,recovery_required,publication_length;
    char card_id[70],checked_card_id[70];
    u32 nonce,phase,failure,transaction_kind,activation_crc;
    u32 transaction_token[4];
    char legacy_names[64][CATALOG_NAME_SIZE];

};
_Static_assert(sizeof(struct library_resident)<=SERVICES_LIBRARY_OWNER_SIZE,"resident owner allocation");
static int library_resident_card_unchanged(struct library_resident *state);
static struct library_objects library_resident_objects(struct library_resident *state,u8 *scratch,u32 capacity)
{
    struct library_objects objects={&state->owner,
        (int (*)(const char *,u32))(__UINTPTR_TYPE__)RT_FS_CREATE,
        (int (*)(const char *,u32 *))(__UINTPTR_TYPE__)RT_FS_OPENDIR,
        (int (*)(u32,void *))(__UINTPTR_TYPE__)RT_FS_READDIR,
        (int (*)(u32))(__UINTPTR_TYPE__)RT_FS_CLOSEDIR,scratch,capacity};
    return objects;
}
/* Presence is established by a successful directory enumeration. A broken
   activation record cannot downgrade a previously managed library. */
static int library_resident_activation(struct library_resident *state)
{
    struct library_objects objects=library_resident_objects(state,state->content,sizeof(state->content));
    int present=0;
    if(library_objects_find(&objects,"C:\\GRKIT","LIB.ACT",&present)) return -1;
    if(!present) return 0;
    u32 stat[16]={0},count=0;
    if(state->owner.fs.stat("C:\\GRKIT\\LIB.ACT",stat) || stat[2]!=LIBRARY_ACTIVATION_BYTES) return -1;
    int handle=state->owner.fs.open("C:\\GRKIT\\LIB.ACT",0,0);
    if(handle<0) return -1;
    int failed=state->owner.fs.read((u32)handle,state->content,LIBRARY_ACTIVATION_BYTES,&count);
    int closed=state->owner.fs.close((u32)handle);
    return !failed && !closed && library_activation_validate(state->content,count,&state->activation_crc) ? 1 : -1;
}
static int library_resident_unactivated_slot(struct library_resident *state,u32 slot)
{
    u32 stat[16]={0};
    if(state->owner.fs.stat(library_owner_path(slot),stat) || stat[2]>LIBRARY_STORE_SLOT_BYTES) return LIBRARY_STORE_IO;
    int handle=state->owner.fs.open(library_owner_path(slot),0,0);
    if(handle<0) return LIBRARY_STORE_IO;
    u8 header[32]={0},commit[16]={0}; u32 offset=0; int failed=0;
    while(offset<stat[2]) {
        u32 count=stat[2]-offset,read=0;
        if(count>sizeof(state->content)) count=sizeof(state->content);
        if(state->owner.fs.read((u32)handle,state->content,count,&read) || read!=count) { failed=1; break; }
        for(u32 i=0;i<count;++i) {
            u32 at=offset+i;
            if(at<32) header[at]=state->content[i];
            if(at>=LIBRARY_STORE_COMMIT_OFFSET) commit[at-LIBRARY_STORE_COMMIT_OFFSET]=state->content[i];
        }
        offset+=count;
    }
    int closed=state->owner.fs.close((u32)handle);
    if(failed || closed) return LIBRARY_STORE_IO;
    return (offset>=12 && library_store_word(header+8)>=2) ||
        (offset>=LIBRARY_STORE_COMMIT_OFFSET+8 && library_store_word(commit+4)>=2) ? LIBRARY_STORE_RECOVERY_REQUIRED : LIBRARY_STORE_OK;
}
static int library_resident_pre_activation(struct library_resident *state,int slots[2])
{
    if(library_resident_activation(state)!=0) return LIBRARY_STORE_RECOVERY_REQUIRED;
    struct library_objects objects=library_resident_objects(state,state->content,sizeof(state->content));
    for(u32 i=0;i<2;++i) {
        if(library_objects_find(&objects,"C:\\GRKIT",i?"LIB1.BIN":"LIB0.BIN",&slots[i])) return LIBRARY_STORE_IO;
        if(slots[i]) { int status=library_resident_unactivated_slot(state,i); if(status) return status; }
    }
    return LIBRARY_STORE_OK;
}
/* Only an unactivated, never-mutated namespace may resume provisioning.
   Original EFFECTS files remain the boot source until LIB.ACT is durable. */
static int library_resident_provision(struct library_resident *state)
{
    if(library_owner_target_admit(state)) return LIBRARY_OWNER_UNVERIFIED_DURABILITY;
    struct library_objects objects=library_resident_objects(state,state->content,sizeof(state->content));
    int slots[2]={0,0},directory=0;
    int check=library_resident_pre_activation(state,slots);
    if(check) return check;
    if(!library_resident_card_unchanged(state)) return LIBRARY_STORE_STALE;
    int status=library_objects_find(&objects,"C:\\GRKIT","CONTENT",&directory);
    if(status) return status;
    if(!directory && ((int (*)(const char *))(__UINTPTR_TYPE__)RT_FS_MKDIR)("C:\\GRKIT\\CONTENT")) return LIBRARY_STORE_IO;
    for(u32 i=0;i<sizeof(state->content);++i) state->content[i]=0;
    for(u32 slot=0;slot<2;++slot) {
        if(!library_resident_card_unchanged(state)) return LIBRARY_STORE_STALE;
        int handle=slots[slot] ? state->owner.fs.open(library_owner_path(slot),0,0) : objects.create(library_owner_path(slot),0);
        if(handle<0) return LIBRARY_STORE_IO;
        int failed=0;
        for(u32 offset=0;offset<LIBRARY_STORE_SLOT_BYTES;) {
            u32 count=LIBRARY_STORE_SLOT_BYTES-offset,written=0;
            if(count>sizeof(state->content)) count=sizeof(state->content);
            if(state->owner.fs.write((u32)handle,state->content,count,&written) || written!=count) { failed=1; break; }
            offset+=count;
        }
        int closed=state->owner.fs.close((u32)handle);
        if(failed || closed || state->owner.fs.durable(state)) return LIBRARY_STORE_IO;
    }
    return LIBRARY_STORE_OK;
}
static int library_resident_activate(struct library_resident *state)
{
    if(state->owner.view.generation!=1 || library_resident_activation(state)!=0 || !library_resident_card_unchanged(state)) return LIBRARY_STORE_RECOVERY_REQUIRED;
    u8 bytes[LIBRARY_ACTIVATION_BYTES];
    library_activation_encode(bytes,state->owner.view.checksum);
    int handle=((int (*)(const char *,u32))(__UINTPTR_TYPE__)RT_FS_CREATE)("C:\\GRKIT\\LIB.ACT",0);
    if(handle<0) return LIBRARY_STORE_IO;
    u32 written=0;
    int failed=state->owner.fs.write((u32)handle,bytes,sizeof(bytes),&written);
    int closed=state->owner.fs.close((u32)handle);
    if(failed || closed || written!=sizeof(bytes) || state->owner.fs.durable(state)) return LIBRARY_STORE_IO;
    return library_resident_activation(state)==1 && state->activation_crc==state->owner.view.checksum ? LIBRARY_STORE_OK : LIBRARY_STORE_RECOVERY_REQUIRED;
}
static int library_resident_grf(const u8 *bytes,u32 length)
{ return grkit_grf_validate(bytes,length); }
static __attribute__((noinline)) int library_resident_manifest(const u8 *bytes,u32 length,struct library_manifest *manifest)
{ return library_manifest_open(bytes,length,manifest); }
static int library_resident_contents(void *context,const u8 *bytes,u32 length)
{
    struct library_resident *state=context;
    struct library_manifest manifest;
    if(!library_resident_card_unchanged(state) || !library_resident_manifest(bytes,length,&manifest)) return 1;
    for(u32 i=0;i<manifest.entries;++i) {
        struct library_manifest_entry entry;
        library_manifest_get(&manifest,i,&entry);
        if(entry.flags&2U) continue;
        char path[32]; u32 stat[16]={0},count=0;
        library_object_path(entry.hash,path);
        if(state->owner.fs.stat(path,stat) || (stat[2]!=GRKIT_GRF_SIZE && stat[2]!=GRKIT_GRF_MAX_SIZE)) return 1;
        int handle=state->owner.fs.open(path,0,0);
        if(handle<0) return 1;
        int failed=state->owner.fs.read((u32)handle,state->content,stat[2],&count);
        int closed=state->owner.fs.close((u32)handle);
        if(failed || closed || count!=stat[2] || !library_content_matches(&entry,state->content,count,library_resident_grf)) return 1;
    }
    return !library_resident_card_unchanged(state);
}
static void library_resident_legacy_path(const char *name,char path[32])
{
    const char *prefix="C:\\GRKIT\\EFFECTS\\"; u32 i=0,j=0;
    while(prefix[i]) { path[i]=prefix[i]; ++i; }
    do { path[i++]=name[j]; } while(name[j++]);
}
static int library_resident_legacy_read(struct library_resident *state,const char *name,u32 *length)
{
    char path[32]; u32 stat[16]={0},count=0;
    library_resident_legacy_path(name,path);
    if(state->owner.fs.stat(path,stat) || (stat[2]!=GRKIT_GRF_SIZE && stat[2]!=GRKIT_GRF_MAX_SIZE)) return 0;
    int handle=state->owner.fs.open(path,0,0); if(handle<0) return 0;
    int failed=state->owner.fs.read((u32)handle,state->content,stat[2],&count);
    int closed=state->owner.fs.close((u32)handle);
    if(failed || closed || count!=stat[2] || !library_resident_grf(state->content,count)) return 0;
    *length=count; return 1;
}
static __attribute__((noinline)) int library_resident_loaded_matches(struct library_resident *state,u32 row,u32 length)
{
    if(!state->telemetry.entries[row].loaded) return 1;
    u8 digest[32];
    if(!library_sha256(state->content,length,digest)) return 0;
    for(u32 i=0;i<state->telemetry.record_count;++i)
        if(library_store_equal(state->telemetry.entries[row].id,state->telemetry.records[i].id,32))
            return library_store_equal(digest,state->telemetry.records[i].hash,32);
    return 0;
}
static __attribute__((noinline)) int library_resident_seed_candidate(struct library_resident *state,volatile u32 *h)
{
    if(!look_telemetry_copy((volatile const struct look_telemetry *)services_word(h,SERVICES_LOOK_TELEMETRY),&state->telemetry) ||
       state->telemetry.count>64 || state->telemetry.record_count>40) return 0;
    for(u32 i=0;i<64;++i) state->legacy_names[i][0]=0;
    struct library_objects objects=library_resident_objects(state,state->owner.scratch,sizeof(state->owner.scratch));
    u32 directory=0;
    if(objects.open_dir("C:\\GRKIT\\EFFECTS",&directory)) return 0;
    int complete=0;
    for(u32 i=0;i<4096;++i) {
        int result=objects.read_dir(directory,state->owner.scratch);
        if(result==1) { complete=1; break; }
        if(result) break;
        u32 n=0; while(n<13 && state->owner.scratch[n]) ++n;
        if(!catalog_name((const char *)state->owner.scratch,n)) continue;
        char name[CATALOG_NAME_SIZE]; for(u32 j=0;j<=n;++j) name[j]=(char)state->owner.scratch[j];
        u32 length;
        if(!library_resident_legacy_read(state,name,&length)) continue;
        for(u32 row=0;row<state->telemetry.count;++row) {
            if(!library_store_equal(state->telemetry.entries[row].id,state->content+GRKIT_GRF_ID,32)) continue;
            if(state->legacy_names[row][0]) { objects.close_dir(directory); return 0; }
            for(u32 j=0;j<=n;++j) state->legacy_names[row][j]=name[j];
        }
    }
    if(objects.close_dir(directory) || !complete) return 0;
    struct library_seed seed;
    if(!library_seed_begin(&seed,state->owner.candidate,sizeof(state->owner.candidate))) return 0;
    for(u32 position=0;position<state->telemetry.count;++position) {
        u32 row=0,length,code=0;
        while(row<state->telemetry.count && state->telemetry.entries[row].position!=position) ++row;
        if(row==state->telemetry.count || !state->legacy_names[row][0] ||
           !library_resident_legacy_read(state,state->legacy_names[row],&length) ||
           !library_resident_loaded_matches(state,row,length)) return 0;
        for(u32 i=0;i<state->telemetry.record_count;++i)
            if(library_store_equal(state->telemetry.entries[row].id,state->telemetry.records[i].id,32)) code=state->telemetry.records[i].code;
        if(!library_seed_add(&seed,state->telemetry.entries[row].id,state->content,length,
            state->telemetry.entries[row].enabled,code,library_resident_grf)) return 0;
    }
    if(!library_seed_finish(&seed,&state->owner.length)) return 0;
    return ((volatile const struct look_telemetry *)services_word(h,SERVICES_LOOK_TELEMETRY))->revision==state->telemetry.revision;
}
static struct library_card_scratch *library_resident_card_scratch(struct library_resident *state)
{ return (struct library_card_scratch *)(((__UINTPTR_TYPE__)state->card_scratch+63U)&~(__UINTPTR_TYPE__)63U); }
static int library_resident_card(struct library_resident *state,char out[70])
{
    return library_card_target_read_evidence(state->card_evidence,library_resident_card_scratch(state)) &&
        library_card_identity(state->card_evidence+4,state->card_evidence+20,out);
}
static int library_resident_card_unchanged(struct library_resident *state)
{
    return library_resident_card(state,state->checked_card_id) &&
        library_store_equal((const u8 *)state->card_id,(const u8 *)state->checked_card_id,70);
}
static int library_resident_pin(void *context)
{
    struct library_resident *state=context;
    volatile u32 *h=(volatile u32 *)(__UINTPTR_TYPE__)state->header;
    volatile const struct look_telemetry *live=(volatile const struct look_telemetry *)services_word(h,SERVICES_LOOK_TELEMETRY);
    volatile struct control_mailbox *box=(volatile struct control_mailbox *)SERVICES_MAP(SERVICES_CONTROL_ADDRESS(h[GRKIT_HOST_SERVICES/4]));
    if((live->seq&1U) || live->flags || live->revision!=state->telemetry.revision ||
       box->generation!=state->metadata.generation) return 0;
    for(u32 i=0;i<4;++i) if(box->epoch[i]!=state->metadata.epoch[i]) return 0;
    return library_resident_card_unchanged(state);
}
static int library_resident_load(void *context,u32 index,const u8 **bytes,u32 *length)
{
    struct library_resident *state=context;
    struct library_manifest_entry entry;
    if(!library_manifest_get(&state->manifest,index,&entry)) return 0;
    struct library_objects objects=library_resident_objects(state,state->content,sizeof(state->content));
    if(library_object_read(&objects,&entry,library_resident_grf)) return 0;
    *bytes=state->content; *length=library_store_word(state->content+GRKIT_GRF_BYTES);
    return 1;
}
static int library_resident_recover(struct library_resident *state)
{
    struct library_store_view view; int status=LIBRARY_STORE_IO;
    if(!library_owner_submit(&state->owner,LIBRARY_OWNER_RECOVER,0,0,0)) return LIBRARY_STORE_IO;
    library_owner_poll(&state->owner);
    if(!library_owner_result(&state->owner,&status,&view)) return LIBRARY_STORE_IO;
    state->status=(u32)status; state->recovered=!status;
    return status;
}
static __attribute__((noinline)) u32 library_resident_begin_frame(struct library_resident *state,
    struct control_request *request,u32 now,u32 *length)
{
    struct library_snapshot_io io={state,library_resident_load,library_resident_pin};
    u8 token[16];
    if(state->nonce==0xffffffffU) return CONTROL_UNSUPPORTED;
    ++state->nonce;
    for(u32 i=0;i<4;++i) library_store_put(token+4*i,i ? state->metadata.epoch[i]^now : state->nonce);
    return library_snapshot_begin(&state->snapshot,&state->metadata,token,&io,now,
        request->payload,sizeof(request->payload),length) ? CONTROL_EMPTY : CONTROL_CHANGED;
}
static u32 library_resident_begin_publication(struct library_resident *,volatile u32 *,volatile struct control_mailbox *,u32,u32);
static u32 library_resident_bootstrap_start(struct library_resident *,volatile u32 *,volatile struct control_mailbox *,u32);
static u32 library_resident_snapshot(struct library_resident *state,volatile u32 *h,
    volatile struct control_mailbox *box,struct control_request *request,u32 now,u32 *length)
{
    struct library_snapshot_io io={state,library_resident_load,library_resident_pin};
    *length=0;
    if(request->command==GRKIT_CONTROL_SNAPSHOT_END)
        return library_snapshot_end(&state->snapshot,&io,now,request->payload,request->length) ? CONTROL_EMPTY : CONTROL_CHANGED;
    if(request->command==GRKIT_CONTROL_SNAPSHOT_READ)
        return library_snapshot_read(&state->snapshot,&state->metadata,&io,now,request->payload,request->length,
            request->payload,sizeof(request->payload),length) ? CONTROL_EMPTY : CONTROL_CHANGED;
    if(request->length) return CONTROL_UNSUPPORTED;
    if(state->snapshot.active || state->apply.active) return CONTROL_BUSY;
    if(!library_resident_card(state,state->card_id)) return CONTROL_CHANGED;
    int activation=library_resident_activation(state);
    if(activation<0) return CONTROL_INVALID;
    if(!activation) return library_resident_bootstrap_start(state,h,box,now);
    if(library_resident_recover(state) || (state->owner.view.generation==1 && state->owner.view.checksum!=state->activation_crc)) return CONTROL_INVALID;
    if(!library_resident_card_unchanged(state) ||
       !library_resident_manifest(state->owner.scratch,state->owner.view.length,&state->manifest) ||
       !look_telemetry_copy((volatile const struct look_telemetry *)services_word(h,SERVICES_LOOK_TELEMETRY),&state->telemetry))
        return CONTROL_CHANGED;
    state->metadata.card_id=state->card_id;
    state->metadata.generation=state->owner.view.generation;
    state->metadata.supported_features=GRKIT_FEATURE_POST|GRKIT_FEATURE_SELECTIVE|GRKIT_FEATURE_HUE|
        GRKIT_FEATURE_HUE_LEVELS|GRKIT_FEATURE_NATIVE_LOOK|GRKIT_FEATURE_FN_DEFAULTS;
    /* Admit only the target whose complete owner/publication path is verified.
       Porting a reader or compiling another target does not enable writes. */
#if defined(GRKIT_TARGET_GR3_221)
    state->metadata.hot_reload=!library_owner_target_admit(0);
#else
    state->metadata.hot_reload=0;
#endif
    state->metadata.monochrome_only=NATIVE_MONOCHROME();
    state->metadata.official=0; state->metadata.official_count=0;
    for(u32 i=0;i<4;++i) state->metadata.epoch[i]=box->epoch[i];
    u32 result=library_snapshot_from_telemetry(&state->manifest,&state->telemetry,&state->metadata_scratch,&state->metadata);
    if(result==LIBRARY_TELEMETRY_NEEDS_RECONCILE) {
        if(state->owner.view.generation==0xffffffffU ||
           !library_telemetry_reconcile(&state->manifest,&state->telemetry,state->owner.candidate,sizeof(state->owner.candidate))) return CONTROL_CHANGED;
        state->owner.length=state->owner.view.length;
        return library_resident_begin_publication(state,h,box,now,2);
    }
    if(result!=LIBRARY_TELEMETRY_OK) return CONTROL_CHANGED;
    box->generation=state->owner.view.generation;
    return library_resident_begin_frame(state,request,now,length);
}

static int library_resident_token(volatile const u32 token[4],const u32 expected[4])
{ for(u32 i=0;i<4;++i) if(token[i]!=expected[i]) return 0; return 1; }
static void library_resident_publication_request(volatile struct look_publish *publish,u32 request)
{ CONTROL_BARRIER(); publish->request=request; CONTROL_BARRIER(); }
static __attribute__((noinline)) u32 library_resident_begin_publication(struct library_resident *state,
    volatile u32 *h,volatile struct control_mailbox *box,u32 now,u32 kind)
{
    volatile struct look_publish *publish=(volatile struct look_publish *)services_word(h,SERVICES_LOOK_PUBLISH);
    if(publish->prepared || state->nonce==0xffffffffU) return CONTROL_UNSUPPORTED;
    ++state->nonce;
    state->transaction_token[0]=state->nonce;
    for(u32 i=1;i<4;++i) state->transaction_token[i]=box->epoch[i]^now;
    library_resident_publication_request(publish,0);
    for(u32 i=0;i<4;++i) publish->token[i]=state->transaction_token[i];
    publish->action=kind==3 ? state->apply_action : LOOK_PUBLISH_METADATA;
    publish->manifest=(u32)(__UINTPTR_TYPE__)state->owner.candidate;
    publish->manifest_length=state->owner.length;
    publish->select_off=0; publish->profile=0; publish->length=0; publish->generation=kind==1 ? 1 : state->owner.view.generation+1;
    if(kind==3) {
        publish->select_off=state->apply.select_off;
        if(state->apply_action!=LOOK_PUBLISH_METADATA) {
            struct library_manifest candidate; struct library_manifest_entry entry;
            if(!library_resident_manifest(state->owner.candidate,state->owner.length,&candidate) ||
               !library_manifest_get(&candidate,state->apply_index,&entry)) return CONTROL_INVALID;
            publish->profile=(u32)(__UINTPTR_TYPE__)state->content;
            publish->length=state->publication_length; publish->enabled=entry.flags&1U;
            for(u32 i=0;i<20;++i) publish->name[i]=entry.name[i];
        }
    }
    state->transaction_kind=kind; state->phase=1; state->failure=0;
    library_resident_publication_request(publish,LOOK_PUBLISH_PREPARE);
    return CONTROL_ACCEPTED;
}
static __attribute__((noinline)) u32 library_resident_bootstrap_start(struct library_resident *state,
    volatile u32 *h,volatile struct control_mailbox *box,u32 now)
{
    int slots[2]={0,0};
    if(!library_resident_card(state,state->card_id) || library_resident_pre_activation(state,slots)) return CONTROL_UNSUPPORTED;
    if(!library_resident_card_unchanged(state) || !library_resident_seed_candidate(state,h) ||
       !library_resident_manifest(state->owner.candidate,state->owner.length,&state->manifest)) return CONTROL_UNSUPPORTED;
    return library_resident_begin_publication(state,h,box,now,1);
}
static __attribute__((noinline)) int library_resident_seed_store(struct library_resident *state)
{
    if(!library_resident_card_unchanged(state)) return LIBRARY_STORE_STALE;
    int status=library_resident_provision(state);
    if(status) return status;
    for(u32 row=0;row<state->telemetry.count;++row) {
        if(!library_resident_card_unchanged(state)) return LIBRARY_STORE_STALE;
        int index=library_telemetry_entry(&state->manifest,state->telemetry.entries[row].id);
        u32 length;
        struct library_manifest_entry entry;
        if(index<0 || !library_manifest_get(&state->manifest,(u32)index,&entry) ||
           !library_resident_legacy_read(state,state->legacy_names[row],&length)) return LIBRARY_STORE_IO;
        struct library_objects objects=library_resident_objects(state,state->owner.scratch,sizeof(state->owner.scratch));
        status=library_object_install_repair(&objects,&entry,state->content,length,library_resident_grf,1);
        if(status) return status;
    }
    if(!library_resident_card_unchanged(state)) return LIBRARY_STORE_STALE;
    if(!library_owner_submit(&state->owner,LIBRARY_OWNER_SEED,0,state->owner.candidate,state->owner.length)) return LIBRARY_STORE_INVALID;
    library_owner_poll(&state->owner);
    struct library_store_view view;
    if(!library_owner_result(&state->owner,&status,&view)) return LIBRARY_STORE_IO;
    if(!status && !library_resident_card_unchanged(state)) status=LIBRARY_STORE_STALE;
    if(!status) status=library_resident_activate(state);
    if(!status && !library_resident_card_unchanged(state)) status=LIBRARY_STORE_STALE;
    state->status=(u32)status; state->recovered=!status;
    return status;
}
static __attribute__((noinline)) int library_resident_reconcile_store(struct library_resident *state)
{
    if(!library_resident_card_unchanged(state)) return LIBRARY_STORE_STALE;
    if(!library_owner_submit(&state->owner,LIBRARY_OWNER_COMMIT,state->owner.view.generation,
        state->owner.candidate,state->owner.length)) return LIBRARY_STORE_INVALID;
    library_owner_poll(&state->owner);
    struct library_store_view view; int status=LIBRARY_STORE_IO;
    if(!library_owner_result(&state->owner,&status,&view)) return LIBRARY_STORE_IO;
    state->status=(u32)status; state->recovered=!status;
    return status;
}
/* Once PREPARE has acknowledged, the App owns a converted unpublished record.
   Object readback may now reuse the old manifest buffer; the candidate remains
   immutable until the App acknowledges COMMIT or CANCEL. */
static __attribute__((noinline)) int library_resident_apply_store(struct library_resident *state)
{
    u32 generation=state->owner.view.generation,length=state->owner.length;
    if(!library_resident_card_unchanged(state)) return LIBRARY_STORE_STALE;
    if(state->apply_action!=LOOK_PUBLISH_METADATA && state->apply.content_length) {
        struct library_manifest candidate; struct library_manifest_entry entry;
        if(!library_resident_manifest(state->owner.candidate,state->owner.length,&candidate) ||
           !library_manifest_get(&candidate,state->apply_index,&entry)) return LIBRARY_STORE_INVALID;
        /* Check every retained identity before enumeration reuses scratch.
           Tombstones and full-hash prefix collisions also protect the path. */
        u32 unreferenced=1;
        for(u32 i=0;i<state->manifest.entries;++i)
            if(library_store_equal(library_manifest_record(&state->manifest,i)+52,entry.hash,4)) unreferenced=0;
        struct library_objects objects=library_resident_objects(state,state->owner.scratch,sizeof(state->owner.scratch));
        int status=library_object_install_repair(&objects,&entry,state->content,state->apply.content_length,library_resident_grf,unreferenced);
        if(status) return status;
    }
    int status=library_resident_reconcile_store(state);
    if(!status) {
        if(library_resident_card_unchanged(state)) return 0;
        state->recovery_required=1; return LIBRARY_STORE_STALE;
    }
    /* A failed close/readback can follow a durable commit. Recover its actual
       identity before deciding whether the prepared runtime may be cancelled. */
    if(!library_resident_card_unchanged(state) || library_resident_recover(state) || !library_resident_card_unchanged(state)) {
        state->recovery_required=1; return status;
    }
    state->owner.length=length;
    if(state->owner.view.generation==generation+1 && state->owner.view.length==length &&
       library_store_equal(state->owner.scratch,state->owner.candidate,length)) return 0;
    u8 recovered_hash[32];
    if(state->owner.view.generation!=generation ||
       !library_sha256(state->owner.scratch,state->owner.view.length,recovered_hash) ||
       !library_store_equal(recovered_hash,state->apply.before_hash,32)) state->recovery_required=1;
    return status;
}
static __attribute__((noinline)) int library_resident_apply_context(struct library_resident *state,
    volatile u32 *h,volatile struct control_mailbox *box)
{
    if(!library_resident_card(state,state->card_id) ||
       library_resident_activation(state)!=1 || library_resident_recover(state) ||
       (state->owner.view.generation==1 && state->owner.view.checksum!=state->activation_crc) ||
       state->owner.view.generation==0xffffffffU || !library_resident_card_unchanged(state) ||
       !library_resident_manifest(state->owner.scratch,state->owner.view.length,&state->manifest) ||
       !look_telemetry_copy((volatile const struct look_telemetry *)services_word(h,SERVICES_LOOK_TELEMETRY),&state->telemetry)) return 0;
    state->metadata.card_id=state->card_id; state->metadata.generation=state->owner.view.generation;
    for(u32 i=0;i<4;++i) state->metadata.epoch[i]=box->epoch[i];
    if(library_snapshot_from_telemetry(&state->manifest,&state->telemetry,&state->metadata_scratch,&state->metadata)!=LIBRARY_TELEMETRY_OK) return 0;
    box->generation=state->owner.view.generation;
    return 1;
}
static __attribute__((noinline)) u32 library_resident_apply_begin(struct library_resident *state,
    volatile u32 *h,volatile struct control_mailbox *box,struct control_request *request,u32 now)
{
    u8 before_hash[32],card_hash[32];
    if(state->snapshot.active || state->apply.active || !library_resident_apply_context(state,h,box) ||
       !library_sha256(state->owner.scratch,state->owner.view.length,before_hash) ||
       !library_sha256((const u8 *)state->card_id,69,card_hash)) return CONTROL_CHANGED;
    return library_apply_admit(&state->apply,request->payload,request->length,state->metadata.epoch,
        card_hash,state->owner.view.generation,before_hash,now) ? CONTROL_EMPTY : CONTROL_CHANGED;
}
/* Enabling one pending entry needs its existing immutable profile admitted by
   App. It is a runtime load, not a content upload or a second storage writer. */
static __attribute__((noinline)) int library_resident_pending_load(struct library_resident *state,
    const struct library_manifest *candidate)
{
    state->publication_length=state->apply.content_length;
    if(state->apply_action!=LOOK_PUBLISH_METADATA) return 1;
    u32 pending;
    if(!library_pending_index(&state->manifest,candidate,&state->telemetry,&pending)) return 0;
    if(pending==256) return 1;
    struct library_manifest_entry entry;
    if(!library_manifest_get(candidate,pending,&entry)) return 0;
    struct library_objects objects=library_resident_objects(state,state->content,sizeof(state->content));
    if(library_object_read(&objects,&entry,library_resident_grf) || !library_resident_card_unchanged(state)) return 0;
    state->publication_length=library_store_word(state->content+GRKIT_GRF_BYTES);
    state->apply_action=LOOK_PUBLISH_REPLACE; state->apply_index=pending;
    return 1;
}
static __attribute__((noinline)) u32 library_resident_apply_commit(struct library_resident *state,
    volatile u32 *h,volatile struct control_mailbox *box,struct control_request *request,u32 now)
{
    struct library_manifest candidate;
    /* pin validates the independently acquired card, epoch, generation and
       camera revision. The App repeats the revision check after taking guard. */
    if(!library_resident_pin(state) ||
       !library_resident_manifest(state->owner.candidate,state->apply.manifest_length,&candidate) ||
       !library_candidate_classify(&state->manifest,&candidate,state->content,state->apply.content_length,
           library_resident_grf,&state->apply_action,&state->apply_index)) return CONTROL_CHANGED;
    if(!library_resident_pending_load(state,&candidate)) return CONTROL_UNSUPPORTED;
    if(state->telemetry.selected && !state->apply.select_off) {
        int index=library_candidate_index(&candidate,state->telemetry.selected_id);
        if(index<0 || (library_store_word(library_manifest_record(&candidate,(u32)index)+84)&2U)) return CONTROL_CHANGED;
    }
    if(!library_apply_seal(&state->apply,request->payload,request->length,now)) return CONTROL_INVALID;
    state->owner.length=state->apply.manifest_length;
    u32 result=library_resident_begin_publication(state,h,box,now,3);
    if(result!=CONTROL_ACCEPTED) library_apply_discard(&state->apply);
    return result;
}
static __attribute__((noinline)) u32 library_resident_apply(struct library_resident *state,
    volatile u32 *h,volatile struct control_mailbox *box,struct control_request *request,u32 now,u32 *length)
{
    *length=0;
    if(state->snapshot.active || state->recovery_required) return CONTROL_INVALID;
    if(request->command==GRKIT_CONTROL_APPLY_ABORT)
        return library_apply_abort(&state->apply,request->payload,request->length,now) ? CONTROL_EMPTY : CONTROL_CHANGED;
    if(request->command==GRKIT_CONTROL_APPLY_COMMIT)
        return library_resident_apply_commit(state,h,box,request,now);
    if(request->command==GRKIT_CONTROL_APPLY_BEGIN) {
        u32 result=library_resident_apply_begin(state,h,box,request,now);
        if(result) return result;
    }
    if(!library_resident_pin(state)) { library_apply_discard(&state->apply); return CONTROL_CHANGED; }
    struct library_apply_buffers buffers={state->owner.candidate,state->content,sizeof(state->owner.candidate),sizeof(state->content)};
    if(!library_apply_write(&state->apply,&buffers,request->payload,request->length,now,library_resident_grf,request->payload)) return CONTROL_INVALID;
    *length=128; return CONTROL_EMPTY;
}
static __attribute__((noinline)) u32 library_resident_transaction(struct library_resident *state,volatile u32 *h,
    volatile struct control_mailbox *box,struct control_request *request,u32 now)
{
    volatile struct look_publish *publish=(volatile struct look_publish *)services_word(h,SERVICES_LOOK_PUBLISH);
    u32 expected=state->phase==1 ? LOOK_PUBLISH_PREPARE : state->phase==3 ? LOOK_PUBLISH_COMMIT : LOOK_PUBLISH_CANCEL;
    if(publish->done!=expected || !library_resident_token(publish->acknowledged,state->transaction_token)) return CONTROL_ACCEPTED;
    CONTROL_BARRIER();
    if(state->phase==1) {
        volatile const struct look_telemetry *live=(volatile const struct look_telemetry *)services_word(h,SERVICES_LOOK_TELEMETRY);
        if(publish->result || (live->seq&1U) || live->flags || live->revision!=state->telemetry.revision) {
            state->status=publish->result;
            state->failure=CONTROL_CHANGED; state->phase=4;
            library_resident_publication_request(publish,LOOK_PUBLISH_CANCEL);
            return CONTROL_ACCEPTED;
        }
        int status=state->transaction_kind==1 ? library_resident_seed_store(state) :
            state->transaction_kind==3 ? library_resident_apply_store(state) : library_resident_reconcile_store(state);
        if(state->recovery_required) {
            /* Keep the prepared runtime guarded until a reboot can reconstruct
               the durable manifest. Never publish an uncertain generation. */
            state->phase=0;
            control_complete(box,request->sequence,CONTROL_INVALID,box->generation);
            return CONTROL_INVALID;
        }
        if(status) {
            state->status=(u32)status; state->failure=CONTROL_INVALID; state->phase=4;
            library_resident_publication_request(publish,LOOK_PUBLISH_CANCEL);
        } else {
            /* Snapshot reconciliation captures the already published catalog without changing
               it. Release its guard rather than rewriting camera settings. */
            state->phase=state->transaction_kind==3 ? 3 : 6;
            library_resident_publication_request(publish,state->phase==3 ? LOOK_PUBLISH_COMMIT : LOOK_PUBLISH_CANCEL);
        }
        return CONTROL_ACCEPTED;
    }
    u32 result=state->failure,response_length=0;
    if(state->phase==3 || state->phase==6) {
        if(publish->result) {
            state->recovery_required=1; state->phase=0;
            control_complete(box,request->sequence,CONTROL_INVALID,box->generation);
            return CONTROL_INVALID;
        }
        state->phase=0;
        if(state->transaction_kind==3) {
            int valid=library_resident_card_unchanged(state);
            for(u32 i=0;i<4;++i) if(box->epoch[i]!=state->metadata.epoch[i]) valid=0;
            if(!valid) state->recovery_required=1;
            else box->generation=state->owner.view.generation;
            result=valid ? CONTROL_EMPTY : CONTROL_INVALID;
        } else result=library_resident_snapshot(state,h,box,request,now,&response_length);
    } else {
        if(publish->prepared) { state->recovery_required=1; result=CONTROL_INVALID; }
        state->phase=0;
    }
    if(state->transaction_kind==3) library_apply_discard(&state->apply);
    box->owner_poll=state->snapshot.active || state->apply.active || state->phase;
    CONTROL_CLEAN(&box->magic,64);
    if(response_length) control_complete_response(box,request->sequence,result,box->generation,request->payload,response_length);
    else control_complete(box,request->sequence,result,box->generation);
    return result;
}

#endif
A32_ENTRY u32 menu_library_owner(u32 array)
{
#if (defined(GRKIT_TARGET_GR3_221) || defined(GRKIT_TARGET_GR4_121)) && defined(__arm__)
    if(!library_owner_target_resident(0) || array<0x53000000U || array>0x6ffffc00U || (array&3U))
        return CONTROL_UNSUPPORTED;
    /* The five-word trampoline calls with LR=array+16. Neither an App timer
       nor an arbitrary direct caller may enter the resident dispatcher. */
    if((u32)(__UINTPTR_TYPE__)__builtin_return_address(0)!=array+16U) return CONTROL_UNSUPPORTED;
    volatile u32 *h=menu_header();
    if(h[GRKIT_HOST_MAGIC/4]!=GRKIT_HOST_MAGIC_VALUE || !h[GRKIT_HOST_SERVICES/4]) return CONTROL_INVALID;
    volatile struct control_mailbox *box=(volatile struct control_mailbox *)SERVICES_MAP(SERVICES_CONTROL_ADDRESS(h[GRKIT_HOST_SERVICES/4]));
    struct library_resident *state=(struct library_resident *)services_word(h,SERVICES_LIBRARY_OWNER);
    if(box->owner_state!=(u32)(__UINTPTR_TYPE__)state || box->owner_entry!=((u32 *)array)[252]) return CONTROL_INVALID;
    if(state->magic && state->magic!=0x4c4f574eU) return CONTROL_BAD_CONTEXT;
    if(state->magic!=0x4c4f574eU) {
        struct library_owner_fs fs=library_owner_target_fs();
        if(!library_owner_init(&state->owner,&fs,state,library_owner_target_resident,library_resident_contents)) return CONTROL_INVALID;
        state->header=(u32)(__UINTPTR_TYPE__)h;
        state->recovered=0; state->status=0; state->magic=0x4c4f574eU;
    }
    if(state->header!=(u32)(__UINTPTR_TYPE__)h) return CONTROL_BAD_CONTEXT;
    u32 now;
    if(!library_clock_ms(&now)) return CONTROL_UNSUPPORTED;
    library_snapshot_expire(&state->snapshot,now);
    library_apply_expire(&state->apply,now);
    box->owner_poll=state->snapshot.active || state->apply.active || state->phase;
    CONTROL_CLEAN(&box->magic,64);
    CONTROL_INVALIDATE(&box->request,64);
    if(box->command<GRKIT_CONTROL_SNAPSHOT_BEGIN || box->command>GRKIT_CONTROL_CARD_IDENTITY) return CONTROL_EMPTY;
    u32 epoch[4]; for(u32 i=0;i<4;++i) epoch[i]=box->epoch[i];
    struct control_request *request=(struct control_request *)services_word(h,SERVICES_CONTROL_CANDIDATE);
    if(state->phase) return library_resident_transaction(state,h,box,request,now);
    u32 result=control_acquire(box,epoch,request);
    if(result==CONTROL_EMPTY) return result;
    if(result==CONTROL_ACCEPTED && state->recovery_required) result=CONTROL_INVALID;
    if(result==CONTROL_ACCEPTED && request->command==GRKIT_CONTROL_CARD_IDENTITY) {
        struct library_card_scratch *scratch=(struct library_card_scratch *)
            (((__UINTPTR_TYPE__)state->card_scratch+63U)&~(__UINTPTR_TYPE__)63U);
        if(request->length==0 && library_card_target_read_evidence(state->card_evidence,scratch)) {
            control_complete_response(box,request->sequence,CONTROL_EMPTY,box->generation,state->card_evidence,532);
            return CONTROL_EMPTY;
        }
        result=CONTROL_UNSUPPORTED;
    }
    u32 response_length=0;
    if(result==CONTROL_ACCEPTED && request->command>=GRKIT_CONTROL_SNAPSHOT_BEGIN && request->command<=GRKIT_CONTROL_SNAPSHOT_END)
        result=library_resident_snapshot(state,h,box,request,now,&response_length);
    if(result==CONTROL_ACCEPTED && request->command>=GRKIT_CONTROL_APPLY_BEGIN && request->command<=GRKIT_CONTROL_APPLY_ABORT)
        result=library_resident_apply(state,h,box,request,now,&response_length);
    if(result==CONTROL_ACCEPTED && !state->phase) result=CONTROL_UNSUPPORTED;
    box->owner_poll=state->snapshot.active || state->apply.active || state->phase;
    CONTROL_CLEAN(&box->magic,64);
    if(state->phase) return CONTROL_ACCEPTED;
    if(response_length) {
        control_complete_response(box,request->sequence,result,box->generation,request->payload,response_length);
        return result;
    }
    control_complete(box,request->sequence,result,box->generation);
    return result;
#else
    (void)array;
    return CONTROL_UNSUPPORTED;
#endif
}
#endif
