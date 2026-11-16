#ifndef GRKIT_LIBRARY_OBJECTS_H
#define GRKIT_LIBRARY_OBJECTS_H
#include "library_owner.h"
#include "library_content.h"
/* Immutable files use a bounded 8.3 hash prefix. A prefix collision is a
   conflict, never permission to overwrite an existing generation. */
#define LIBRARY_OBJECT_CONFLICT 70
struct library_objects {
    struct library_owner *owner;
    int (*create)(const char *,u32);
    int (*open_dir)(const char *,u32 *);
    int (*read_dir)(u32,void *);
    int (*close_dir)(u32);
    u8 *scratch;
    u32 capacity;
};
static void library_object_path(const u8 hash[32],char path[32])
{
    const char *prefix="C:\\GRKIT\\CONTENT\\",*hex="0123456789ABCDEF";
    u32 i=0;
    while(prefix[i]) { path[i]=prefix[i]; ++i; }
    for(u32 j=0;j<4;++j) { path[i++]=hex[hash[j]>>4]; path[i++]=hex[hash[j]&15]; }
    path[i++]='.'; path[i++]='G'; path[i++]='R'; path[i++]='F'; path[i]=0;
}
/* A failed stat does not prove absence. Enumerate the parent through RTOS;
   errors, malformed names and unbounded directories all fail closed. */
static int library_objects_find(struct library_objects *objects,const char *folder,const char *name,int *exists)
{
    u32 directory=0;
    if(objects->open_dir(folder,&directory)) return LIBRARY_STORE_IO;
    int status=LIBRARY_STORE_IO; *exists=0;
    for(u32 i=0;i<4096;++i) {
        int result=objects->read_dir(directory,objects->scratch);
        if(result==1) { status=LIBRARY_STORE_OK; break; }
        if(result) break;
        u32 j=0;
        while(j<768 && objects->scratch[j]) ++j;
        if(j==768) break;
        u32 k=0;
        while(k<j && name[k]) {
            u8 c=objects->scratch[k]; if(c>='a' && c<='z') c-=32;
            if(c!=(u8)name[k]) break;
            ++k;
        }
        if(k==j && !name[k]) { *exists=1; status=LIBRARY_STORE_OK; break; }
    }
    return objects->close_dir(directory) ? LIBRARY_STORE_IO : status;
}
static int library_object_read(struct library_objects *objects,const struct library_manifest_entry *entry,
                               int (*validate)(const u8 *,u32))
{
    if(!objects || !objects->owner || !entry || !validate || !objects->scratch ||
       objects->capacity<GRKIT_GRF_MAX_SIZE) return LIBRARY_STORE_INVALID;
    struct library_owner *owner=objects->owner;
    if(!owner->is_resident(owner->context)) return LIBRARY_OWNER_NOT_RESIDENT;
    char path[32]; u32 stat[16]={0},count=0;
    library_object_path(entry->hash,path);
    if(owner->fs.stat(path,stat)) return LIBRARY_STORE_IO;
    if(stat[2]!=GRKIT_GRF_SIZE && stat[2]!=GRKIT_GRF_MAX_SIZE) return LIBRARY_OBJECT_CONFLICT;
    int handle=owner->fs.open(path,0,0);
    if(handle<0) return LIBRARY_STORE_IO;
    int failed=owner->fs.read((u32)handle,objects->scratch,stat[2],&count);
    int closed=owner->fs.close((u32)handle);
    if(failed || closed || count!=stat[2]) return LIBRARY_STORE_IO;
    return library_content_matches(entry,objects->scratch,count,validate) ? LIBRARY_STORE_OK : LIBRARY_OBJECT_CONFLICT;
}
/* bytes must not alias scratch: enumeration and readback deliberately replace
   scratch. The resident owns the namespace; no Linux writer is supported. */
/* allow_partial_repair is evidence supplied by the owner, not by HTTP: App
   holds the prepare guard and no current manifest identity (including removed
   identities) references this 8-hex filename prefix. Before activation the
   owner may instead prove that only an interrupted initial seed exists.
   Even with that evidence, only an exact incomplete prefix may be extended. */
static int library_object_install_repair(struct library_objects *objects,const struct library_manifest_entry *entry,
                                  const u8 *bytes,u32 length,int (*validate)(const u8 *,u32),u32 allow_partial_repair)
{
    if(!objects || !objects->owner || !objects->create || !objects->open_dir || !objects->read_dir ||
       !objects->close_dir || !objects->scratch || objects->capacity<GRKIT_GRF_MAX_SIZE ||
       !library_content_matches(entry,bytes,length,validate)) return LIBRARY_STORE_INVALID;
    __UINTPTR_TYPE__ input=(__UINTPTR_TYPE__)bytes, scratch=(__UINTPTR_TYPE__)objects->scratch;
    if((input>=scratch && input-scratch<objects->capacity) ||
       (scratch>=input && scratch-input<length)) return LIBRARY_STORE_INVALID;
    struct library_owner *owner=objects->owner;
    if(!owner->is_resident(owner->context)) return LIBRARY_OWNER_NOT_RESIDENT;
    if(!owner->fs.durable || !owner->fs.admit_write || owner->fs.admit_write(owner->context))
        return LIBRARY_OWNER_UNVERIFIED_DURABILITY;
    char path[32]; int exists=0;
    library_object_path(entry->hash,path);
    int status=library_objects_find(objects,"C:\\GRKIT\\CONTENT",path+17,&exists);
    if(status) return status;
    int handle;
    if(exists) {
        if(!allow_partial_repair) return library_object_read(objects,entry,validate);
        u32 stat[16]={0},count=0;
        if(owner->fs.stat(path,stat)) return LIBRARY_STORE_IO;
        u32 previous=stat[2];
        if(previous>=length) {
            int result=library_object_read(objects,entry,validate);
            if(result) return result;
            /* A previous repair may have written all bytes but failed its
               durability barrier. Re-establish it without rewriting bytes. */
            if(owner->fs.durable(owner->context)) return LIBRARY_STORE_IO;
            return library_object_read(objects,entry,validate);
        }
        handle=owner->fs.open(path,0,0);
        if(handle<0) return LIBRARY_STORE_IO;
        int failed=previous && owner->fs.read((u32)handle,objects->scratch,previous,&count);
        int closed=owner->fs.close((u32)handle);
        if(failed || closed || count!=previous) return LIBRARY_STORE_IO;
        /* A valid short-form GRF is a complete different object, not a torn
           long-form upload. A damaged/mismatching prefix is not repairable. */
        if((previous==GRKIT_GRF_SIZE && validate(objects->scratch,previous)) ||
           !library_store_equal(objects->scratch,bytes,previous)) return LIBRARY_OBJECT_CONFLICT;
        if(owner->fs.stat(path,stat) || stat[2]!=previous) return LIBRARY_STORE_IO;
        /* MODE_RDWR starts at offset zero, preserves the directory entry and
           extends the short file. Never truncate, remove, or rename it. */
        handle=owner->fs.open(path,0,0);
    } else handle=objects->create(path,0);
    if(handle<0) return LIBRARY_STORE_IO;
    u32 count=0;
    int failed=owner->fs.write((u32)handle,(void *)bytes,length,&count);
    int closed=owner->fs.close((u32)handle);
    if(failed || closed || count!=length || owner->fs.durable(owner->context)) return LIBRARY_STORE_IO;
    return library_object_read(objects,entry,validate);
}
/* Ordinary callers cannot acquire repair authority accidentally. */
static int library_object_install(struct library_objects *objects,const struct library_manifest_entry *entry,
                                  const u8 *bytes,u32 length,int (*validate)(const u8 *,u32))
{ return library_object_install_repair(objects,entry,bytes,length,validate,0); }
#endif
