#ifndef GRKIT_FAST_MANAGED_PRESENCE_H
#define GRKIT_FAST_MANAGED_PRESENCE_H
/* Unlike the legacy cache, managed-library admission must distinguish absence
   from inaccessible storage. Only a complete directory read proves absence. */
static int fast_managed_find(const char *folder,const char *name,u8 *scratch)
{
    u32 handle=0;
    if(FS_OPENDIR(folder,&handle)) return -1;
    int found=-1;
    for(u32 row=0;row<4096;++row) {
        /* Poison the name field so an unterminated/partial result fails. */
        for(u32 i=0;i<768;++i) scratch[i]=0xff;
        int status=FS_READDIR(handle,scratch);
        if(status==1) { found=0; break; }
        if(status) break;
        u32 n=0; while(n<768 && scratch[n]) ++n;
        if(!n || n==768) break;
        u32 i=0;
        while(i<n && name[i]) {
            u8 c=scratch[i]; if(c>='a' && c<='z') c-=32;
            if(c!=(u8)name[i]) break;
            ++i;
        }
        if(i==n && !name[i]) { found=1; break; }
    }
    return FS_CLOSEDIR(handle) ? -1 : found;
}
static int fast_managed_presence(const char *name,u8 *scratch)
{
    int parent=fast_managed_find("C:\\","GRKIT",scratch);
    return parent==1 ? fast_managed_find("C:\\GRKIT",name,scratch) : parent;
}
#endif
