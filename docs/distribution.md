# 배포 방식

현재 배포는 `SDIR-windows-x64-setup.exe`와 `SDIR-windows-x64-portable.zip`을 제공한다. 설치형과 포터블 모두 `dist/2026-11-20-search-ux`에 있다. 포터블은 압축을 풀고 `SDIR.exe`를 실행한다. 런타임을 확인하려면 `Start-SDIR.cmd`를 사용한다.

ZIP에는 실행 파일·문서·라이선스·검사 스크립트·`portable.flag`만 들어간다. 사용자 폴더 목록·검색 기록·WebView 데이터는 넣지 않는다. 플래그가 있으면 옆의 `data` 폴더에 설정과 WebView 데이터를 저장하므로 앱 폴더 전체를 옮길 때 설정도 함께 이동한다. 기존 AppData 설정이 있고 아직 휴대용 설정이 없다면 첫 실행에서 SDIR 설정만 복사하며 기존 파일은 보존한다. 플래그 없는 기존 `dist/SDIR.exe`는 `%LOCALAPPDATA%\SDIR` 설정을 계속 사용한다. `SDIR_PROFILE_DIR` 환경 변수는 두 방식보다 우선한다.

다른 PC의 드라이브 문자·사용자 경로가 다르면 저장한 폴더가 존재하지 않을 수 있다. 설정은 복원되며 해당 패널에서 접근 오류를 표시하고 유효한 경로로 이동할 수 있다. Windows x64와 Microsoft Edge WebView2 Evergreen Runtime이 필요하다. [Microsoft 배포 안내](https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution)에 따라 공유 런타임을 사용한다. 런타임 설치 파일은 이 ZIP에 포함하지 않는다.

Windows x64 릴리스 빌드는 `x86_64-pc-windows-msvc` 대상을 명시하고 MSVC CRT를 정적으로 링크한다. `scripts/build-windows.ps1`는 기본적으로 기존 출력 폴더 `dist`를 사용하며, 이전 배포본과 나란히 시험할 때는 `-OutputDirectory .\dist-static-crt-stage`처럼 별도 폴더를 지정할 수 있다.

공개 배포본을 Smart App Control이 활성화된 Windows 11에서 신뢰할 수 있도록 하려면 Microsoft Trusted Root Program의 인증 기관(CA)이 발급한 코드 서명 인증서로 앱 실행 파일, 설치 프로그램, 제거 프로그램과 함께 배포하는 바이너리를 서명한다. [Microsoft Smart App Control 안내](https://learn.microsoft.com/en-us/windows/security/application-security/application-control/smart-app-control)를 따른다.

| 방식 | 적합한 경우 | 차이 |
|---|---|---|
| 포터블 ZIP | 개인 사용, USB/여러 위치에서 실행, 시험 배포 | 압축 해제·폴더 복사로 배포; 업데이트는 앱 종료 후 새 실행 파일·문서 교체, `data` 보존 |
| 사용자별 설치형 | 일반 사용자에게 널리 배포 | 시작 메뉴·바로가기·업데이트·프로그램 제거 등록을 표준 설치 도구로 제공할 수 있음 |

두 배포 파일에는 같은 실행 파일이 들어 있다. 설치형은 Inno Setup을 사용해 사용자별로 설치하고 시작 메뉴와 제거 항목을 등록한다. 같은 배포본의 새 설치·재설치·제거와 설치된 앱 실행은 스마트 앱 컨트롤이 꺼진 Windows에서 확인했다. [설치 및 재설치 안내](windows-installer.md)

Windows PowerShell에서 `scripts/package-portable.ps1`로 생성한다. ZIP 내부 실행 파일 SHA-256과 필수 파일·사용자 데이터 제외 여부를 검사한 뒤 최종 파일로 옮긴다.
