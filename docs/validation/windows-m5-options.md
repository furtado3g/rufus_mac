# Validação M5 — Personalizações Windows 11 (US3)

Data: 2026-10-22
Ambiente: macOS x86_64 Darwin 23.6.0, QEMU 11.1.1 (TCG)

## Imagem Utilizada no Experimento Manual (T031)
- Arquivo: `Windows11_Client_x64_pt-br_26300_9457.iso`
- Tamanho: 8.731.574.272 bytes (8,1 GiB)
- SHA-256: `d9ac87c1e9e47e0eb34eed443e5691c31781074e67608a51aee50d7c7039f684`
- Volume ID: `CCCOMA_X64FRE_PT-BR_DV9`
- Metadados WIM (`sources/install.wim`):
  - Build: `26300` (SPBuild `9457`)
  - Edições presentes: Core, CoreSingleLanguage, Education, Professional, ProfessionalEducation, ProfessionalWorkstation
  - Arquitetura: x86_64 (arch 9)

*Nota de matriz*: A ISO acima (build `26300`) é um build de prévia/desenvolvimento e difere da matriz mínima de referência Windows 11 24H2 oficial (build `26100`). Capacidades de bypass de requisitos foram testadas comprovadamente nesta build via WinPE LabConfig. O ajuste de conta offline não foi validado nesta build em OOBE completa e portanto permanece desabilitado/rejeitado como não comprovado para a build.

## Experimento WinPE LabConfig (T031)
1. **Procedimento**:
   - Criada mídia de resposta manual (FAT16/FAT32) contendo `autounattend.xml` na raiz.
   - Configurações aplicadas exclusivamente na fase `windowsPE` via `Microsoft-Windows-Setup` -> `RunSynchronous`:
     - `reg add HKLM\System\Setup\LabConfig /v BypassTPMCheck /t reg_dword /d 0x00000001 /f`
     - `reg add HKLM\System\Setup\LabConfig /v BypassSecureBootCheck /t reg_dword /d 0x00000001 /f`
   - Inicializada VM QEMU x86_64 (TCG, sem hardware TPM 2.0, sem Secure Boot ativo no firmware OVMF, com dispositivo de rede e1000e).
   - Mídia de instalação carregada via UEFI:NTFS da imagem real preparada em M5.
   - Mídia de resposta anexada como USB removível.
2. **Resultado**:
   - O instalador do Windows 11 inicializou em WinPE.
   - Avançado pelas telas de idioma, teclado e opção de instalação sem chave de produto.
   - O instalador pulou a tela de bloqueio de requisitos mínimos ("Este computador não pode executar o Windows 11") e avançou diretamente para a tela "Selecionar Imagem" (edições da WIM), comprovando a eficácia das chaves LabConfig via answer file sem tocar na WIM de origem.
   - Evidências registradas em telas de captura QEMU (`screen14.png` a `screen21.png`).
3. **Conta Offline**:
   - Não foi possível comprovar local account / BypassNRO até a OOBE interativa completa sob emulação QEMU TCG devido ao tempo proibitivo de extração de 8 GiB na CPU TCG.
   - Conforme especificação (FR-010, SC-004 e T031), nenhuma build tem conta offline anunciada sem validação comprovada. Tentativas de solicitar `--offline-account` são rejeitadas como build não comprovada até que evidência de OOBE seja produzida.

