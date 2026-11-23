#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <bcrypt.h>
#include <sddl.h>
#include <shellapi.h>
#include <shlobj.h>
#include <algorithm>
#include <cwctype>
#include <cstdint>
#include <iterator>
#include <string>
#include <vector>
#include "payload.h"

namespace {
constexpr DWORD APP_ONLY_WIZARD_EXIT = 42;

struct Handle {
    HANDLE value;
    explicit Handle(HANDLE raw = nullptr) : value(raw) {}
    ~Handle() { if (*this) CloseHandle(value); }
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
    explicit operator bool() const { return value && value != INVALID_HANDLE_VALUE; }
};

void check(bool ok, const wchar_t* message) {
    if (!ok) throw std::wstring(message);
}

bool equals(const std::wstring& a, const std::wstring& b) {
    return _wcsicmp(a.c_str(), b.c_str()) == 0;
}

std::wstring modulePath() {
    std::vector<wchar_t> path(32768);
    DWORD count = GetModuleFileNameW(nullptr, path.data(), static_cast<DWORD>(path.size()));
    check(count > 0 && count < path.size(), L"설치 프로그램 경로를 읽을 수 없습니다.");
    return {path.data(), count};
}

std::wstring programFiles() {
    PWSTR path = nullptr;
    check(SUCCEEDED(SHGetKnownFolderPath(FOLDERID_ProgramFilesX64, 0, nullptr, &path)) && path,
          L"Windows의 Program Files 폴더를 확인하지 못했습니다.");
    std::wstring result(path);
    CoTaskMemFree(path);
    return result;
}

bool isElevated() {
    return IsUserAnAdmin() != FALSE;
}

std::wstring tokenSid(HANDLE process) {
    HANDLE raw = nullptr;
    check(OpenProcessToken(process, TOKEN_QUERY, &raw) != FALSE, L"설치를 시작한 사용자 정보를 읽지 못했습니다.");
    Handle token(raw);
    DWORD bytes = 0;
    GetTokenInformation(token.value, TokenUser, nullptr, 0, &bytes);
    check(bytes >= sizeof(TOKEN_USER), L"Windows 사용자 SID를 확인하지 못했습니다.");
    std::vector<unsigned char> data(bytes);
    check(GetTokenInformation(token.value, TokenUser, data.data(), bytes, &bytes) != FALSE,
          L"Windows 사용자 SID를 확인하지 못했습니다.");
    PWSTR sid = nullptr;
    check(ConvertSidToStringSidW(reinterpret_cast<TOKEN_USER*>(data.data())->User.Sid, &sid) != FALSE,
          L"Windows 사용자 SID를 변환할 수 없습니다.");
    std::wstring result(sid);
    LocalFree(sid);
    check(result.rfind(L"S-1-5-21-", 0) == 0 && result.size() <= 54 &&
          result.find_first_not_of(L"0123456789-", 2) == std::wstring::npos, L"지원되는 Windows 사용자 SID가 아닙니다.");
    return result;
}

uint64_t creationTime(HANDLE process) {
    FILETIME start{}, end{}, kernel{}, user{};
    check(GetProcessTimes(process, &start, &end, &kernel, &user) != FALSE, L"설치 프로세스 시작 시각을 확인하지 못했습니다.");
    return (uint64_t(start.dwHighDateTime) << 32) | start.dwLowDateTime;
}

uint64_t number(const wchar_t* text) {
    check(*text && std::all_of(text, text+wcslen(text), [](wchar_t c){return c>=L'0'&&c<=L'9';}), L"설치 인수가 잘못되었습니다.");
    return std::stoull(text);
}

std::wstring sha256(const unsigned char* data, ULONG size) {
    unsigned char bytes[32];
    check(BCryptHash(BCRYPT_SHA256_ALG_HANDLE, nullptr, 0, const_cast<PUCHAR>(data), size, bytes, sizeof(bytes)) == 0,
          L"설치 파일 검증에 실패했습니다.");
    static constexpr wchar_t hex[] = L"0123456789abcdef";
    std::wstring result;
    for (const auto value : bytes) { result += hex[value >> 4]; result += hex[value & 15]; }
    return result;
}

struct Payload {
    const unsigned char* data;
    DWORD size;
};

Payload payload(int id, DWORD expectedSize, const wchar_t* expectedHash) {
    auto handle = FindResourceW(nullptr, MAKEINTRESOURCEW(id), RT_RCDATA);
    auto resource = handle ? LoadResource(nullptr, handle) : nullptr;
    auto data = resource ? static_cast<const unsigned char*>(LockResource(resource)) : nullptr;
    auto size = handle ? SizeofResource(nullptr, handle) : 0;
    check(data && size == expectedSize && sha256(data, size) == expectedHash, L"내장 설치 파일의 무결성 확인에 실패했습니다.");
    return {data, size};
}

std::wstring fileHash(const std::wstring& path) {
    Handle file(CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr));
    if (!file) return {};
    BY_HANDLE_FILE_INFORMATION info{};
    if (!GetFileInformationByHandle(file.value, &info) ||
        (info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY|FILE_ATTRIBUTE_REPARSE_POINT)) ||
        info.nFileSizeHigh || !info.nFileSizeLow || info.nFileSizeLow>64*1024*1024) return {};
    std::vector<unsigned char> data(info.nFileSizeLow);
    DWORD count = 0;
    if (!ReadFile(file.value,data.data(),info.nFileSizeLow,&count,nullptr)||count!=info.nFileSizeLow) return {};
    return sha256(data.data(),count);
}

std::wstring registryText(const wchar_t* key, const wchar_t* name) {
    wchar_t buffer[1024]{};
    DWORD bytes=sizeof(buffer);
    if (RegGetValueW(HKEY_LOCAL_MACHINE,key,name,RRF_RT_REG_SZ|RRF_RT_REG_EXPAND_SZ|RRF_NOEXPAND|RRF_SUBKEY_WOW6464KEY,
                     nullptr,buffer,&bytes)!=ERROR_SUCCESS) return {};
    return buffer;
}

DWORD registryNumber(const wchar_t* key, const wchar_t* name) {
    DWORD value=0,bytes=sizeof(value);
    if (RegGetValueW(HKEY_LOCAL_MACHINE,key,name,RRF_RT_REG_DWORD|RRF_SUBKEY_WOW6464KEY,nullptr,&value,&bytes)!=ERROR_SUCCESS) return MAXDWORD;
    return value;
}

enum class ServiceState { absent, current, prior, conflict };
ServiceState serviceState(const std::wstring& sid) {
    const std::wstring root=programFiles()+L"\\SDIR Index Service";
    const std::wstring image=root+L"\\SDIRIndexService.exe";
    constexpr wchar_t ownerKey[]=L"SOFTWARE\\SDIR\\IndexService";
    constexpr wchar_t imageKey[]=L"SYSTEM\\CurrentControlSet\\Services\\SDIRIndex";
    const DWORD attributes=GetFileAttributesW(root.c_str());
    const std::wstring owner=registryText(ownerKey,L"OwnerSid");
    const auto manager=OpenSCManagerW(nullptr,nullptr,SC_MANAGER_CONNECT);
    check(manager!=nullptr,L"Windows 서비스 관리자를 열 수 없습니다.");
    const auto service=OpenServiceW(manager,L"SDIRIndex",SERVICE_QUERY_STATUS);
    const DWORD serviceError=service?ERROR_SUCCESS:GetLastError();
    SERVICE_STATUS status{};
    const bool statusOk=service&&QueryServiceStatus(service,&status);
    if(service)CloseServiceHandle(service);
    CloseServiceHandle(manager);
    if(!service && serviceError==ERROR_SERVICE_DOES_NOT_EXIST && attributes==INVALID_FILE_ATTRIBUTES && owner.empty()) return ServiceState::absent;
    if(!statusOk || attributes==INVALID_FILE_ATTRIBUTES || (attributes&FILE_ATTRIBUTE_REPARSE_POINT) ||
       !(attributes&FILE_ATTRIBUTE_DIRECTORY) || owner!=sid ||
       !equals(registryText(imageKey,L"ImagePath"),L"\""+image+L"\"") ||
       !equals(registryText(imageKey,L"ObjectName"),L"LocalSystem") || registryNumber(imageKey,L"Start")!=SERVICE_DEMAND_START ||
       registryNumber(imageKey,L"Type")!=SERVICE_WIN32_OWN_PROCESS) return ServiceState::conflict;
    const auto hash=fileHash(image);
    if(hash==SERVICE_IMAGE_SHA)return ServiceState::current;
    if(!hash.empty()&&hash==PRIOR_SERVICE_SHA&&status.dwCurrentState==SERVICE_STOPPED)return ServiceState::prior;
    return ServiceState::conflict;
}

void linkIndexService() {
    const auto root = programFiles() + L"\\SDIR Index Service";
    constexpr wchar_t key[] =
        L"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\SDIR-Index-Service-BF7D31D4-84CC-482A-8AE5-45C9BEA4100F_is1";
    const auto registered = registryText(key, L"UninstallString");
    check(registered.size() == root.size() + 15 && registered.front() == L'"' && registered.back() == L'"',
          L"색인 서비스의 제거 등록을 확인하지 못했습니다.");
    const auto basename = registered.substr(root.size() + 2, 12);
    const auto image = root + L"\\" + basename;
    check(equals(registered, L"\"" + image + L"\"") && equals(basename.substr(0, 5), L"unins") &&
          equals(basename.substr(8), L".exe") &&
          std::all_of(basename.begin()+5, basename.begin()+8, [](wchar_t digit) { return digit >= L'0' && digit <= L'9'; }) &&
          equals(registryText(key, L"InstallLocation"), root + L"\\") &&
          equals(registryText(key, L"QuietUninstallString"), registered + L" /SILENT"),
          L"색인 서비스의 제거 등록을 확인하지 못했습니다.");
    const auto exeHash = fileHash(image);
    const auto datHash = fileHash(image.substr(0, image.size()-4) + L".dat");
    check(!exeHash.empty() && !datHash.empty(), L"색인 서비스 제거 파일을 확인하지 못했습니다.");
    const auto link = exeHash + L":" + datHash;
    HKEY association = nullptr;
    const auto open = RegCreateKeyExW(HKEY_CURRENT_USER, APP_LINK_KEY, 0, nullptr, 0,
                                     KEY_SET_VALUE | KEY_WOW64_64KEY, nullptr, &association, nullptr);
    check(open == ERROR_SUCCESS, L"SDIR과 색인 서비스의 제거 정보를 저장하지 못했습니다.");
    const auto recorded = RegSetValueExW(association, L"IndexRemover", 0, REG_SZ,
                    reinterpret_cast<const BYTE*>(link.c_str()), static_cast<DWORD>((link.size()+1)*sizeof(wchar_t)));
    RegCloseKey(association);
    check(recorded == ERROR_SUCCESS, L"SDIR과 색인 서비스의 제거 정보를 저장하지 못했습니다.");
}

struct TemporarySetup {
    std::wstring directory, file;
    explicit TemporarySetup(bool elevated, const wchar_t* filename) {
        wchar_t temp[MAX_PATH+1]{};
        if(!elevated) {
            DWORD length=GetTempPathW(MAX_PATH,temp);
            check(length&&length<MAX_PATH,L"Windows 임시 폴더를 확인하지 못했습니다.");
        }
        GUID guid{};
        wchar_t name[40]{};
        check(SUCCEEDED(CoCreateGuid(&guid))&&StringFromGUID2(guid,name,40),L"임시 설치 공간을 준비하지 못했습니다.");
        directory=(elevated?programFiles()+L"\\":std::wstring(temp))+L"SDIR-Setup-"+name;
        file=directory+L"\\"+filename;
        PSECURITY_DESCRIPTOR descriptor=nullptr;
        SECURITY_ATTRIBUTES attributes{sizeof(attributes),nullptr,FALSE};
        if(elevated){
            check(ConvertStringSecurityDescriptorToSecurityDescriptorW(L"O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",SDDL_REVISION_1,&descriptor,nullptr)!=FALSE,
              L"관리자 전용 설치 공간을 준비하지 못했습니다.");
            attributes.lpSecurityDescriptor=descriptor;
        }
        const bool made=CreateDirectoryW(directory.c_str(),elevated?&attributes:nullptr)!=FALSE;
        if(descriptor)LocalFree(descriptor);
        check(made,L"고유한 설치 공간을 만들지 못했습니다.");
    }
    ~TemporarySetup(){DeleteFileW(file.c_str());RemoveDirectoryW(directory.c_str());}
    void extract(const Payload& resource, const wchar_t* expectedHash){
        {
            Handle out(CreateFileW(file.c_str(),GENERIC_WRITE,0,nullptr,CREATE_NEW,FILE_ATTRIBUTE_NORMAL,nullptr));
            DWORD written=0;
            check(out && WriteFile(out.value,resource.data,resource.size,&written,nullptr)&&written==resource.size&&FlushFileBuffers(out.value),
              L"설치 파일을 저장하지 못했습니다.");
        }
        check(fileHash(file)==expectedHash,L"전개한 설치 파일의 해시가 다릅니다.");
    }
};

DWORD waitFor(HANDLE process){
    DWORD exit=ERROR_INSTALL_FAILURE;
    check(WaitForSingleObject(process,INFINITE)==WAIT_OBJECT_0&&GetExitCodeProcess(process,&exit),L"설치 결과를 확인하지 못했습니다.");
    return exit;
}

DWORD spawnInstaller(const TemporarySetup& setup,const std::wstring& arguments){
    std::wstring command=L"\""+setup.file+L"\" "+arguments;
    STARTUPINFOW start{};
    start.cb=sizeof(start);
    PROCESS_INFORMATION info{};
    check(CreateProcessW(setup.file.c_str(),command.data(),nullptr,nullptr,FALSE,0,nullptr,setup.directory.c_str(),&start,&info)!=FALSE,
       L"내장 설치 프로그램을 시작하지 못했습니다.");
    Handle process(info.hProcess), thread(info.hThread);
    return waitFor(process.value);
}

DWORD installService(DWORD parentPid,uint64_t expectedStart){
    check(isElevated(),L"색인 서비스를 설치할 관리자 권한이 없습니다.");
    Handle parent(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|SYNCHRONIZE,FALSE,parentPid));
    check(parent&&creationTime(parent.value)==expectedStart&&WaitForSingleObject(parent.value,0)==WAIT_TIMEOUT,L"원래 설치 프로세스를 확인하지 못했습니다.");
    wchar_t path[32768]{};
    DWORD size=static_cast<DWORD>(std::size(path));
    check(QueryFullProcessImageNameW(parent.value,0,path,&size)&&equals(modulePath(),std::wstring(path,size)),L"원래 통합 설치 프로그램과 일치하지 않습니다.");
    const auto sid=tokenSid(parent.value);
    const auto state=serviceState(sid);
    if(state==ServiceState::current)return 0;
    check(state==ServiceState::absent||state==ServiceState::prior,L"기존 색인 서비스의 소유권 또는 상태를 확인해 주세요.");
    TemporarySetup setup(true,L"SDIR-Index-Setup.exe");
    setup.extract(payload(SERVICE_PAYLOAD_ID,SERVICE_SETUP_BYTES,SERVICE_SETUP_SHA),SERVICE_SETUP_SHA);
    const auto exit=spawnInstaller(setup,L"/VERYSILENT /SUPPRESSMSGBOXES /SP- /NORESTART /SDIR-OWNER-SID="+sid);
    if(exit!=0)return exit;
    check(serviceState(sid)==ServiceState::current,L"설치 후 색인 서비스의 등록과 파일을 확인하지 못했습니다.");
    return 0;
}

DWORD requestElevation(){
    const auto image=modulePath();
    Handle lock(CreateFileW(image.c_str(),GENERIC_READ,FILE_SHARE_READ,nullptr,OPEN_EXISTING,FILE_FLAG_OPEN_REPARSE_POINT,nullptr));
    check(bool(lock),L"통합 설치 프로그램을 고정할 수 없습니다.");
    std::wstring arguments=L"--service-stage "+std::to_wstring(GetCurrentProcessId())+L" "+std::to_wstring(creationTime(GetCurrentProcess()));
    const auto workingDirectory=programFiles();
    SHELLEXECUTEINFOW command{};
    command.cbSize=sizeof(command);
    command.fMask=SEE_MASK_NOCLOSEPROCESS|SEE_MASK_NOASYNC|SEE_MASK_FLAG_NO_UI;
    command.lpVerb=L"runas"; command.lpFile=image.c_str(); command.lpParameters=arguments.c_str();
    command.lpDirectory=workingDirectory.c_str(); command.nShow=SW_SHOWNORMAL;
    if(!ShellExecuteExW(&command))return GetLastError();
    Handle process(command.hProcess);
    check(bool(process),L"관리자 설치 프로세스를 확인하지 못했습니다.");
    return waitFor(process.value);
}
}

int WINAPI wWinMain(HINSTANCE,HINSTANCE,PWSTR,int){
    bool quiet=false;
    PWSTR* args=nullptr;
    try{
        check(SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)!=FALSE,L"Windows 시스템 라이브러리 경로를 설정하지 못했습니다.");
        int count=0;
        args=CommandLineToArgvW(GetCommandLineW(),&count);
        check(args&&count>0,L"설치 인수를 읽지 못했습니다.");
        if(count==4&&std::wstring(args[1])==L"--service-stage"){
            quiet=true;
            auto pid=number(args[2]);
            auto start=number(args[3]);
            check(pid>0&&pid<=MAXDWORD,L"설치 프로세스 식별자가 잘못되었습니다.");
            LocalFree(args); args=nullptr;
            return static_cast<int>(installService(static_cast<DWORD>(pid),start));
        }
        if(count==2&&std::wstring(args[1])==L"--verify-payloads"){
            quiet=true;
            payload(APP_PAYLOAD_ID,APP_SETUP_BYTES,APP_SETUP_SHA);
            payload(SERVICE_PAYLOAD_ID,SERVICE_SETUP_BYTES,SERVICE_SETUP_SHA);
            LocalFree(args); return 0;
        }
        bool service=true, explicitChoice=false;
        for(int i=1;i<count;++i){
            std::wstring arg=args[i];
            if(arg==L"--silent"&&!quiet){quiet=true;continue;}
            if(!explicitChoice&&(arg==L"--app-only"||arg==L"--with-service")){
                explicitChoice=true; service=arg==L"--with-service"; continue;
            }
            throw std::wstring(L"알 수 없는 설치 옵션입니다.");
        }
        LocalFree(args); args=nullptr;
        check(!isElevated(),L"SDIR을 사용할 Windows 계정에서 설치 파일을 일반 실행해 주세요.");
        check(!quiet||explicitChoice,L"자동 설치에는 설치 범위를 지정해 주세요.");
        Handle installLock(CreateMutexW(nullptr, TRUE, APP_INSTALL_MUTEX));
        check(installLock && GetLastError() != ERROR_ALREADY_EXISTS, L"이미 실행 중인 SDIR 설치를 끝내 주세요.");
        TemporarySetup setup(false,L"SDIR-App-Setup.exe");
        setup.extract(payload(APP_PAYLOAD_ID,APP_SETUP_BYTES,APP_SETUP_SHA),APP_SETUP_SHA);
        std::wstring appArguments=quiet?L"/VERYSILENT /SUPPRESSMSGBOXES /SP- /NORESTART":L"/SP- /NORESTART";
        if(!explicitChoice)appArguments+=L" /SDIR-INDEX-OPTION=1 /MERGETASKS=indexservice";
        const auto appExit=spawnInstaller(setup,appArguments);
        if(!explicitChoice&&appExit==APP_ONLY_WIZARD_EXIT)return 0;
        if(appExit!=0)return static_cast<int>(appExit);
        if(!service)return 0;
        const auto state=serviceState(tokenSid(GetCurrentProcess()));
        if(state==ServiceState::current){
            linkIndexService();
            return 0;
        }
        if(state==ServiceState::conflict){
            if(!quiet)MessageBoxW(nullptr,L"앱 설치는 끝났습니다. 기존 색인 서비스가 사용 중이거나 등록이 다릅니다. 서비스를 그대로 유지했습니다.",L"SDIR 색인 서비스",MB_OK|MB_ICONWARNING);
            return ERROR_INSTALL_FAILURE;
        }
        const auto serviceExit=requestElevation();
        if(serviceExit==0){
            check(serviceState(tokenSid(GetCurrentProcess()))==ServiceState::current, L"설치 후 색인 서비스의 소유권을 확인하지 못했습니다.");
            linkIndexService();
        }
        if(serviceExit!=0&&!quiet)MessageBoxW(nullptr,L"앱 설치는 끝났습니다. 색인 서비스 설치는 완료되지 않았습니다.",L"SDIR 색인 서비스",MB_OK|MB_ICONWARNING);
        return static_cast<int>(serviceExit);
    }catch(const std::wstring& error){
        if(args)LocalFree(args);
        if(!quiet)MessageBoxW(nullptr,error.c_str(),L"SDIR 설치",MB_OK|MB_ICONERROR);
        return ERROR_INSTALL_FAILURE;
    }catch(...){
        if(args)LocalFree(args);
        return ERROR_INVALID_PARAMETER;
    }
}
