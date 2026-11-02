// SPDX-License-Identifier: MIT
// Host check of the pipeline-cache export/import format (CacheExport.h): a directory tree becomes the export blob, the blob is written where
// the image would carry it, and importPack rebuilds the tree in a second directory; every file must come back byte for byte, an existing
// file of equal size is kept, and damaged packs import nothing. argv: source dir, destination dir, blob path. No kernel call is made.
#define CACHEEXPORT_FORMAT_ONLY
#include <cstring>
#include "../CacheExport.h"
#include <cstdio>
#include <cstdlib>
#include <sys/wait.h>

#define CHECK(x) do {if(!(x)){std::fprintf(stderr,"line %d: %s\n",__LINE__,#x);std::exit(1);}}while(0)

static std::vector<uint8_t> slurp(const std::string &path) {
    std::vector<uint8_t> out;FILE *f=fopen(path.c_str(),"rb");if(!f)return out;
    uint8_t b[4096];for(size_t n;(n=fread(b,1,sizeof b,f))>0;)out.insert(out.end(),b,b+n);fclose(f);return out;
}
static void spill(const std::string &path,const std::vector<uint8_t> &data) {FILE *f=fopen(path.c_str(),"wb");CHECK(f);CHECK(fwrite(data.data(),1,data.size(),f)==data.size());fclose(f);}

int main(int argc,char **argv) {
    CHECK(argc==4);
    const std::string source=argv[1],dest=argv[2],blobPath=argv[3];
    std::vector<cacheexport::File> files;cacheexport::scan(source,"",files,0);
    CHECK(files.size()>=3);
    const uint64_t sig=cacheexport::signature(files);
    std::vector<cacheexport::File> again;cacheexport::scan(source,"",again,0);
    CHECK(cacheexport::signature(again)==sig);
    std::vector<uint8_t> blob;CHECK(cacheexport::build(source,files,blob));
    /* the export's stream: as long as announced, and written out as the uncompressed export blob for the host's readers */
    std::vector<uint8_t> raw={'S','H','E','X','R','0','0','1',0,0,0,0,0,0,0,0};
    CHECK(cacheexport::stream(source,files,[&](const uint8_t *p,size_t n){raw.insert(raw.end(),p,p+n);return true;}));
    CHECK(raw.size()-16==cacheexport::streamBytes(source,files));
    {const uint32_t n=(uint32_t)(raw.size()-16);std::memcpy(&raw[8],&n,4);}
    spill(blobPath+".raw",raw);
    mkdir(dest.c_str(),0700);mkdir((dest+"-damaged").c_str(),0700);   /* the plugin makes its directory before importing */
    spill(blobPath,blob);
    CHECK(cacheexport::baked().empty());
    CHECK(cacheexport::importPack(blobPath.c_str(),dest)==(int)files.size());
    CHECK(cacheexport::baked().size()==files.size());   /* every entry of a pack read to its end is known as baked */
    for(const auto &f:files)CHECK(slurp(source+"/"+f.path)==slurp(dest+"/"+f.path));
    // Independent processes may populate the same directory at the same time.
    const std::string shared=dest+"-shared";mkdir(shared.c_str(),0700);
    pid_t workers[2];
    for (auto &pid:workers) {
        pid=fork();CHECK(pid>=0);
        if(!pid)_exit(cacheexport::importPack(blobPath.c_str(),shared)>=0?0:1);
    }
    for(auto pid:workers){int status=0;CHECK(waitpid(pid,&status,0)==pid && WIFEXITED(status) && WEXITSTATUS(status)==0);}
    for(const auto &f:files)CHECK(slurp(source+"/"+f.path)==slurp(shared+"/"+f.path));
    std::vector<cacheexport::File> sharedFiles;cacheexport::scan(shared,"",sharedFiles,0);CHECK(sharedFiles.size()==files.size());
    spill(shared+"/in-progress.import.1.0",{1,2});
    sharedFiles.clear();cacheexport::scan(shared,"",sharedFiles,0);CHECK(sharedFiles.size()==files.size());
    CHECK(cacheexport::importPack(blobPath.c_str(),dest)==0);   /* all present with their sizes: nothing rewritten */
    /* damaged packs: truncated, bad magic, flipped compressed byte */
    std::vector<uint8_t> bad=blob;bad.resize(bad.size()/2);spill(blobPath+".t",bad);
    CHECK(cacheexport::importPack((blobPath+".t").c_str(),dest+"-damaged")==-1);
    bad=blob;bad[0]='X';spill(blobPath+".m",bad);CHECK(cacheexport::importPack((blobPath+".m").c_str(),dest+"-damaged")==-1);
    bad=blob;bad[bad.size()-5]^=0x55;spill(blobPath+".f",bad);CHECK(cacheexport::importPack((blobPath+".f").c_str(),dest+"-damaged")==-1);
    CHECK(cacheexport::importPack((blobPath+".missing").c_str(),dest)==0);
    std::vector<cacheexport::File> left;cacheexport::scan(dest+"-damaged","",left,0);CHECK(left.empty());
    CHECK(cacheexport::baked().size()==files.size());   /* damaged packs add nothing */
    /* the export leaves baked files out: only a new file, or one whose size changed, goes out */
    std::vector<cacheexport::File> now;cacheexport::scan(dest,"",now,0);cacheexport::dropBaked(now);CHECK(now.empty());
    spill(dest+"/new-entry",{1,2,3});
    { std::vector<uint8_t> grown=slurp(dest+"/"+files[0].path);grown.push_back(7);spill(dest+"/"+files[0].path,grown); }
    now.clear();cacheexport::scan(dest,"",now,0);cacheexport::dropBaked(now);
    CHECK(now.size()==2);
    std::printf("PASS: cache export %zu files, %zu blob bytes; import exact, idempotent, 3 damaged packs refused, baked files left out of the export\n",files.size(),blob.size());
    return 0;
}
