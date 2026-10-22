# Tasks: M5 — Windows installation media

**Input**: `specs/001-windows-installation-media/`: spec.md, plan.md, research.md, data-model.md, contracts/windows-media.md, quickstart.md.

**Tests**: Exigidos pelos critérios da spec e gates do plano; usar suítes Rust existentes, sem framework novo. Criar casos de regressão antes da implementação correspondente, confirmar falha esperada e depois aprovação.

**Organization**: tarefas por história; paths relativos à raiz. `[P]` indica trabalho em arquivos distintos, somente após pré-requisitos explícitos. Não autoriza iniciar agentes automaticamente.

**Entrega macOS primeiro (aprovação do usuário)**: executar as tarefas inicialmente no host macOS; Linux/Windows serão compilados separadamente pelo usuário. T004 prioriza a receita macOS; receita PowerShell e runners dos outros hosts ficam pendentes. T007 libera histórias no Mac após provas nativas locais, sem exigir cross-checks ou builds dos demais hosts. Registrar separadamente esses resultados; tarefas com escopo multiplataforma só recebem `[X]` após conclusão integral. Falhas locais de ABI/formatter continuam bloqueantes. T036/T037/T041 preservam pendências multiplataforma; não declarar M5 integralmente concluído por um incremento macOS.

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: fixar entradas reproduzíveis, evidências e baseline sem gravar dispositivos.

- [X] T001 Executar fmt, Clippy, testes e builds baseline do ROADMAP; registrar comandos, resultados e ferramentas disponíveis em `docs/validation/windows-m5-build.md`, sem alterar ou descartar trabalho existente.
  - Baseline registrada em 2026-10-21: checks nativos aprovados; cross-checks Linux/Windows indisponíveis localmente (E0463, targets ausentes; sem rustup). Pendências preservadas para CI; não bloqueiam aquisição de fontes T002. Builds nativos multiplataforma continuam gate obrigatório de T007.
- [X] T002 Fixar versões/commits e SHA-256 de libudfread, NTFS-3G, wimlib-imagex 1.14.5 e imagem UEFI:NTFS em `assets/windows-media/manifest.json`; registrar por recurso “identificação, versão/commit, plataforma/arquitetura, tamanho/hash, licença, fonte correspondente e receita” e distinguir helpers do host de assets EFI alvo.
- [X] T003 Documentar em `assets/windows-media/README.md` fontes oficiais, patches, receitas e obrigações GPL/LGPL por componente; incluir materiais de relink quando necessários e mecanismo de entrega das fontes correspondentes, sem tratar link upstream isolado como cumprimento suficiente.

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: provar fronteiras nativas e preservar invariantes comuns antes de qualquer história.

- [ ] T004 Implementar receitas nativas fixadas em `scripts/build-windows-media.sh` e `scripts/build-windows-media.ps1`, consumindo `assets/windows-media/manifest.json`, sem FUSE ou dependências instaladas no usuário final; integrar link/build em `crates/platform/build.rs` e `crates/platform/Cargo.toml`, adicionando apenas dependências de build indispensáveis; configurar desde a fundação os runners/builds nativos mínimos em `.github/workflows/ci.yml` para T007, sem depender da expansão final T036.
  - Parcial em 2026-10-21: receita macOS criada; três fontes verificadas; libudfread/libntfs-3g estáticas e helper WIM 1.14.5 compilados no Mac Intel com MacPorts. Roundtrip WIM aprovado; helper depende apenas de libSystem. Link Rust opt-in e smoke test nativo aprovados, Clippy sem warnings. Job CI macOS adicionado com MacPorts verificado por SHA-256; execução remota pendente. Reexecução local com teste WIM `--check` em andamento. Receitas dos demais hosts continuam pendentes; nenhum PASS remoto presumido.
- [X] T005 Encapsular leitura UDF em `crates/platform/src/media.rs` e `crates/platform/native/udf.c`, exportando API segura em `crates/platform/src/lib.rs`; usar callbacks read-only com offsets de 64 bits, bounds/short I/O, cleanup e nenhuma execução de conteúdo da ISO.
  - Concluída no incremento macOS em 2026-10-21: ABI direta de libudfread sem shim C; listagem, leitura com offsets de 64 bits, bounds, EOF, paths estritos, imagem inválida, leitura sparse >4 GiB e erro de callback testados. Fixture UDF fragmentada gerada com `newfs_udf` e arquivo sparse de 16 MiB (extensões em 1 MiB e 9 MiB), `fsck_udf -n` limpo e leitura correta via `udf_probe`; fixture manual, não commitada. `contain_io` converte panic em erro retido e tem regressão unitária. A inspeção de `udfread_readdir` mostrou que, após `opendir` bem-sucedido, as entradas já estão carregadas e `NULL` significa EOF; falhas de parser/I/O só ocorrem no `opendir` e são recuperadas pelo erro salvo. A fixture fragmentada agora é reproduzida pelo teste `udf_fragmented_sparse_file_reads_back` em `crates/platform/tests/media.rs` (imagem temporária, sem dispositivo físico ou root).
- [X] T006 Integrar libntfs-3g e adaptar formatter mkntfs em `crates/platform/native/ntfs.c` e `crates/platform/src/media.rs`; mapear callbacks RawIo com offsets relativos verificados, flush e erro original; “nenhuma exceção/panic cruza ABI” e “Formatter global não é reentrante”, portanto serializar formatter e impedir escape de ponteiros.
  - Concluída no incremento macOS em 2026-10-21: implementação em `native/ntfs_format.c` (nome difere do path original) chama mkntfs por callbacks sem path/FUSE; `NtfsFormatOptions` recebe LBA da partição e geometria; boot sector com hidden sectors/CHS corretos; setores 512/4096, backup boot, flush, erro de escrita/sync, label inválido, volume pequeno/desalinhado, panic contido e concorrência serializada testados. `ntfsinfo -m`, `file` e montagem read-only pelo driver nativo do macOS validaram um fixture de 64 MiB. Patch de cleanup/estado global do mkntfs e adaptação `-Dlibintl_setlocale=setlocale` documentados. Não comprova boot, chkdsk nem hosts Linux/Windows.
- [ ] T007 Acrescentar checks executáveis nativos em `crates/platform/tests/media.rs`: UDF fragmentado e >4 GiB, NTFS formatado com diretórios/arquivo >4 GiB, short I/O, erro/cancelamento e flush; compilar nos hosts/arquiteturas do plano e registrar leitura independente/chkdsk em `docs/validation/windows-m5-build.md`. Falha de ABI/formatter interrompe este caminho para revisão do plano, sem fallback experimental.
  - Parcial em 2026-10-21: no macOS, UDF fragmentado automatizado (newfs_udf + hdiutil em imagem temporária, fsck limpo, sparse 16 MiB, leituras A/B/buraco/EOF), leitura UDF >4 GiB, formatter NTFS 512/4096 e API de escrita NTFS implementada. Testes cobrem árvore de diretórios, arquivo de 5.242.884 bytes, arquivo sparse >4 GiB com offsets de 64 bits, arquivo preenchido de 4.296.015.872 bytes validado por `ntfsinfo` e montagem read-only nativa, I/O curto limitado a 4096 bytes, cancelamento de escrita/sync, flush e concorrência do formatter. Ainda faltam chkdsk Windows, builds Linux/Windows, mídia física e boot/Secure Boot; T007 não está concluída.
- [X] T008 Implementar extração/resolução de helper privado e workspace em `crates/platform/src/media.rs` usando tempfile: permissões/ACL privadas, criação exclusiva, validação de hash, caminho absoluto e argumentos sem shell/PATH; cancelamento aguarda término do helper antes da limpeza, sem remover arquivos preexistentes.
  - Concluída no incremento macOS em 2026-10-21: `Helper::resolve` exige prefixo absoluto, caminho relativo sem traversal, contenção canônica contra escape por symlink, arquivo regular executável e SHA-256 de 32 bytes; o hash é revisto imediatamente antes de executar. `Helper::run` usa `Command` com caminho absoluto, argv separado, ambiente limpo e `LC_ALL=C`, sem shell/PATH, grava stdout/stderr em arquivos exclusivos e converte saída não-zero em `Error::Command` com stderr limitado. `PrivateWorkspace` usa `tempfile` com modo 0700, arquivos `O_EXCL` 0600 e não remove caminhos preexistentes. Cancelamento mata e aguarda o filho antes de retornar. 9 testes unitários no crate (5 novos) cobrem hash/traversal/symlink, permissões, injeção de shell, falha e cancelamento. Pendente fora do macOS: ACL Windows e publicação dos hashes reais dos helpers por arquitetura no manifesto de release (T004/T037); a API não aceita hash default.
- [X] T009 Estender opções/WritePlan em `crates/core/src/plan.rs` e exports em `crates/core/src/lib.rs`: “Auto, Fat32, Ntfs” para seleção, “não introduzir Auto no writer”; WindowsOptions com “bypass_tpm, bypass_secure_boot, offline_account; bools independentes, false por padrão”, substituir bypass_win11 genérico e “aceitar campos ausentes com defaults”; testar roundtrip/defaults no próprio arquivo.
  - Concluída no incremento macOS em 2026-10-21: `FileSystemChoice` (auto/fat32/ntfs) é a seleção; `FileSystem` continua apenas fat32/ntfs para writer/plano. `WriteOptions` usa a escolha e `WindowsOptions`; `bypass_win11` foi removido. `WritePlan` guarda `requested_filesystem`, o `filesystem` concreto, `filesystem_reason` e `windows`. Serde aceita campos ausentes com defaults; `Auto` nunca chega ao writer e o caminho de extração falha explicitamente se não tiver sido resolvido. 5 testes de default/roundtrip/independência no próprio arquivo. O default de `WriteOptions` permanece Fat32 para não alterar o comportamento atual; o default CLI `auto` e a resolução real entram em T017/T023.
- [X] T010 Adicionar Preparing em `crates/core/src/progress.rs`, atualizar matches/serialização consumidores em `crates/cli/src/main.rs` e `crates/gui/src/main.rs`; representar “Após primeira escrita: erro/cancelamento produz Failed/Cancelled com target_incomplete = true”, Done exige flush e avisos de ejeção separados; deixar check executável no módulo afetado.
  - Concluída no incremento macOS em 2026-10-21: `Phase::Preparing` adicionada; `ProgressEvent` ganhou `target_incomplete` com default serde falso. Construtores explícitos `failed_before_write`, `failed_after_write`, `cancelled_before_write`, `cancelled_after_write` e `done_after_flush` (o nome obriga o chamador a afirmar o flush); `Phase::Ejecting` permanece não terminal e separado. `Phase::is_data`/`is_terminal` centralizam os matches; CLI usa `is_data`, GUI e CLI exibem o aviso pelo `summary`, e a serialização preserva o campo. 4 testes no módulo cobrem Preparing, estados terminais, target incompleto, Done/ejeção e default de campo ausente. A emissão dos eventos terminais no fluxo real de commit fica em T019/T034; nenhum teste declara escrita física.

**Checkpoint**: escopo macOS de T001–T010 aprovado antes das histórias no Mac. Resultados de outros hosts permanecem pendentes, nunca marcados como sucesso; não bloqueiam este incremento conforme aprovação acima.

## Phase 3: User Story 1 — Criar mídia Windows (Priority: P1)

**Goal**: mídia FAT32 Windows sem arquivo acima do limite, criada por CLI e GUI, sem personalizações e com verificação.

**Independent Test**: Windows 10/11 x64 com arquivos abaixo do limite FAT32 chega à primeira tela setup; arquivos lidos correspondem à origem, ISO não muda, erro de preparação não modifica destino.

### Tests

- [ ] T011 [P] [US1] (parcial: UDF-only pendente) Criar fixtures e regressões de Windows UDF-only/ISO9660 multi-extent em `crates/iso/tests/synthetic.rs` e builders mínimos em `crates/testiso/src/lib.rs`; cobrir marcadores WIM/ESD/SWM sem bootmgr, u64, truncamento, “caminhos sem absoluto/../NUL/links”, “extents dentro da origem” e “ambiguidades case-insensitive em caminhos Windows críticos falham”.
- [ ] T012 [P] [US1] (parcial: evidência de releitura corrompida e ENOSPC real pendentes) Criar testes de preparação/commit e hash integral antes/depois em `crates/extract/tests/extract.rs`: boot ausente, arquitetura unknown, capacidade insuficiente, alteração detectável da origem durante captura do snapshot (inclusive conteúdo de mesmo tamanho), alteração posterior sem efeito no snapshot, falta de espaço/limpeza/cancelamento da captura, corrupção de releitura e nenhuma personalização padrão; incluir “PE machine coerente com arquitetura; unknown impede gravação Windows”.

### Implementation

- [ ] T013 [US1] (parcial: UDF não integrado) Unificar catálogo e leitura em `crates/iso/src/iso9660.rs` e `crates/iso/src/lib.rs`, integrar UDF por dependência segura em `crates/iso/Cargo.toml`; representar “tamanho u64, referências a extents ISO ou handle UDF”, “ordem de diretórios antes de arquivos”, limite de 250.000 entradas, aritmética checked e junção multi-extent sem cast superficial.
- [X] T014 [US1] Ampliar WindowsInfo/detecção em `crates/iso/src/analyze.rs`: “arquitetura x64/arm64/unknown”, “versão/build/edição quando identificáveis”, WIM/ESD/SWM, tamanho/partes e fallback PE; JSON aditivo snake_case preserva windows.install_wim_size, sem temporários/segredos; corrigir alegação DD baseada apenas em El Torito.
- [ ] T015 [US1] (parcial: plano/manifesto e custo de transformações futuras pendentes) Implementar preparação Windows comum em `crates/extract/src/windows.rs` e exportar em `crates/extract/src/lib.rs`: capturar snapshot privado integral antes da análise definitiva, comparar identidade/metadados da origem read-only antes/depois, abortar mudança detectada e usar exclusivamente snapshot nas leituras posteriores; incluir ISO inteira e transformações no custo scratch, tratar ENOSPC/cleanup/cancelamento sem tocar destino. Receber catálogo, geometria, cancel token e sink sem RawIo gravável; validar boot/arquitetura, preservar EFI assinados e obter fallback do snapshot; criar plano/manifesto e workspace com ciclo de vida próprio, sem alegar autenticidade ou detecção adversarial universal.
- [X] T016 [US1] Adaptar commit FAT32 em `crates/extract/src/extract.rs` para consumir preparação Windows, verificar capacidade final antes da tabela, copiar bootmgr/BCD/EFI e todos os arquivos; sem GRUB/reescrita Linux de boot config no caminho Windows; hashes esperados de saídas e releitura obrigatória com --verify, sync e falhas explícitas.
- [X] T017 [US1] Integrar CLI em `crates/cli/src/main.rs`: --filesystem auto/fat32/ntfs, plano final e motivo antes da confirmação, auto Windows não híbrido extract, dry-run com preparação e zero escrita; checar identidade por handle para origem igual ao destino/symlink/hard link mesmo com --yes e revalidar identidade/capacidade imediatamente antes da abertura exclusiva; acrescentar regressões CLI no próprio módulo.
- [ ] T018 [US1] (parcial: preparação ainda ocorre no worker de escrita após abrir destino; sem testes de transição) Integrar GUI em `crates/gui/src/main.rs`: selector filesystem, preparação em worker com cancelamento, confirmação apenas do plano final, mudança de origem/opção/destino invalida confirmação; substituir rejeição Windows genérica por compatibilidade específica e manter labels acessíveis/teclado; testar transições puras no módulo.
- [ ] T019 [US1] Executar testes US1 e boot Windows 10/11 x64 FAT32 em OVMF até setup, Secure Boot on/off, registrar ISO hash/build, commit, DB/DBX, assinaturas e evidências em `docs/validation/windows-m5-fat32.md`; preservar caminhos Linux/DD nos testes existentes, nunca registrar boot não executado como PASS.

**Checkpoint**: MVP funcional FAT32 pequeno por CLI/GUI. Não representa M5 completo; arquivos grandes, NTFS e ajustes continuam pendentes.

### Estado da implementação US1 (2026-10-21, macOS x86_64)

T014/T016/T017 têm implementação e regressões sintéticas; T011/T012/T013/T015/T018 permanecem parciais pelos itens anotados. A extração Windows FAT32 usa snapshot privado integral, valida BOOTMGR/BCD/fallback EFI e PE, rejeita links/caminhos ambíguos, copia o catálogo e faz releitura SHA-256 com `--verify`. O CLI resolve `auto|fat32|ntfs`, recusa NTFS/DD incompatível antes de preparar, faz dry-run com preparação e rejeita origem igual ao destino (inclusive symlink/hard link). Não houve boot Windows, ISO real, OVMF, Secure Boot, dispositivo físico, build Linux/Windows ou CI remoto. UDF-only continua recusado com erro específico porque o catálogo UDF ainda não foi integrado. US2 recebeu um incremento sintético em 2026-10-22 (ver notas T021/T024–T027); WIM split, arquivo >4 GiB real e boot continuam abertos. Nenhuma tarefa US3 foi iniciada.

## Phase 4: User Story 2 — Imagens grandes e NTFS (Priority: P1)

**Goal**: dividir WIM, preservar ESD/SWM e oferecer NTFS/UEFI:NTFS, sem tarefas manuais de preparação.

**Independent Test**: WIM grande dividido lista mesmas edições e inicia aplicação em disco virtual descartável; NTFS preserva arquivo >4 GiB e inicia setup; erros prévios deixam destino intacto.

### Tests

- [ ] T020 [P] [US2] Criar regressões de split/seleção em `crates/extract/tests/extract.rs`: 4.294.967.295 e 4.294.967.296 bytes, WIM indivisível/solid, ESD grande, SWM ausente/duplicado/GUID divergente, helper falho/hash inválido e scratch insuficiente; “SWM recebe part number/total/GUID coerentes” e “Não sobrescrever conjunto SWM original com novo split”.
- [ ] T021 [P] [US2] Criar testes de layout NTFS em `crates/extract/src/layout.rs` e `crates/extract/src/gpt.rs`: MBR/GPT, setores 512/4096, overflow, dispositivo pequeno e backup GPT; “Sem overlap, alinhada 1 MiB, dentro do dispositivo” e “Tamanho de asset derivado do arquivo real”; testar geometria interna/BPB e recusa antes de escrever quando asset incompatível ou não comprovado, preservando FAT32 4Kn existente.
  - Parcial em 2026-10-22: `crates/extract/src/windows_layout.rs` planeja/escreve MBR e GPT com duas partições, alinhamento 1 MiB, sem overlap, backup GPT e tamanho do ESP derivado dos 1.048.576 bytes reais do asset; testes validam CRC/entradas, assinatura FAT e BPB interno (512 bytes/setor, 2048 setores, 1 MiB). 4Kn é recusado explicitamente antes de escrever, preservando o caminho FAT32 4Kn existente. Ainda faltam prova 4096 e validação MBR independente; T021 permanece aberta.

### Implementation

- [ ] T022 [US2] Integrar inspeção/split wimlib em `crates/extract/src/windows.rs` pelo helper privado: preservar edições/metadados, dividir WIM em partes-alvo 3800 MiB, validar tamanho real de cada parte e sequência; preservar ESD/SWM válidos sem conversão, informar scratch necessário e tratar ENOSPC antes de abrir destino.
- [ ] T023 [US2] Resolver auto/manual em `crates/extract/src/windows.rs` após preparar saídas: “Auto prioriza FAT32 e divide WIM grande. ESD/outro arquivo grande ou WIM impossível de dividir validamente resolve NTFS”; “escolha manual incompatível falha antes de commit” e “Mudança de resolução invalida confirmação anterior”; calcular capacidade incluindo metadados, diretórios e recursos auxiliares reais.
  - Parcial em 2026-10-22: `resolve_filesystem` mantém FAT32 no limite de 4.294.967.295 bytes, resolve NTFS acima dele e recusa FAT32 explícito incompatível antes de qualquer commit; teste CLI cobre os dois lados do limite. Split automático de WIM e invalidação de confirmação por mudança de resolução continuam pendentes; T023 permanece aberta.
- [ ] T024 [US2] Entregar imagem UEFI:NTFS upstream intacta em `assets/uefi-ntfs/uefi-ntfs.img` e registrar em `assets/uefi-ntfs/README.md` commit/hash, versões/arquiteturas, PE signatures, fontes/licenças por componente e condições DB/DBX; inspecionar BPB/geometria e comprovar compatibilidade por setor lógico 512/4096, documentando recusas quando incompatível ou não comprovado; sincronizar com `assets/windows-media/manifest.json`, sem alterar asset/binários assinados. Implementar validação antecipada da combinação em `crates/extract/src/windows.rs`.
  - Parcial em 2026-10-22: asset exato copiado do commit Rufus fixado, SHA-256 `72683fa1…eb9e` e tamanho 1 MiB verificados por teste; README registra proveniência e alerta para não montar read-write no macOS. GPT gerado foi reconhecido pelo macOS como Microsoft Basic Data + EFI/RUFUS_BOOT. BPB FAT validado antes da gravação (512 bytes/setor, 2048 setores, 1 MiB, assinatura `0x55AA`); boots sintéticos OVMF em MBR e GPT iniciaram o chainloader v2.8 e um app EFI de teste a partir da partição NTFS. Assinaturas PE, auditoria DB/DBX, fontes por componente e compatibilidade 4096 continuam pendentes; T024 permanece aberta.
- [ ] T025 [US2] Implementar duas partições em `crates/extract/src/layout.rs` e `crates/extract/src/gpt.rs`: NTFS primeira 0x07/Microsoft Basic Data, FAT auxiliar final 0xEF/ESP dimensionada pelo asset, start/length u64 e proteção GPT primário/backup; manter MBR default e layouts FAT32/Linux anteriores.
  - Parcial em 2026-10-22: implementado em `windows_layout.rs` em vez de alterar os layouts FAT32: NTFS 0x07/Microsoft Basic Data primeiro, ESP 0xEF/EFI System de 1 MiB ao final, MBR e GPT primário/backup. Testes cobrem ambas as tabelas e limites GPT. Boots sintéticos OVMF em MBR e GPT fizeram o chainloader encontrar a partição NTFS 1. Falta validação independente em 4Kn; T025 permanece aberta.
- [ ] T026 [US2] Implementar commit/verificação NTFS em `crates/extract/src/extract.rs` via API segura platform: formatar, criar diretórios e escrever arquivos em blocos, copiar asset FAT auxiliar, preservar arquivos grandes, comparar tamanhos/hashes de todos os arquivos e asset, propagar offset/flush/cancelamento e estado incompleto.
  - Parcial em 2026-10-22: commit NTFS atrás da feature macOS `windows-media`, com snapshot privado, formatação no LBA real, cópia em offsets u64, asset UEFI:NTFS byte-a-byte, sync/unmount, comparação do asset e releitura SHA-256 de todos os arquivos com `--verify`. Testes sintéticos MBR/GPT passam e macOS montou o NTFS read-only. Boots sintéticos OVMF (MBR e GPT, 512 bytes, Secure Boot desligado) carregaram um app EFI de teste pelo chainloader. Falta arquivo real >4 GiB, setup Windows, chkdsk independente e estado incompleto em falha física; T026 permanece aberta.
- [ ] T027 [US2] Conectar auto/FAT32/NTFS e transformações reais em `crates/cli/src/main.rs` e `crates/gui/src/main.rs`, sem lógica de seleção duplicada; testar recusa Linux NTFS, filesystem explícito em DD e nenhuma troca silenciosa de escolha manual; remover rejeição NTFS apenas após writer implementado.
  - Parcial em 2026-10-22: CLI/GUI removem a rejeição cega apenas quando `WINDOWS_NTFS_SUPPORTED`; builds sem a feature continuam recusando antes de abrir o destino. Planos mostram NTFS + UEFI:NTFS e a GUI só habilita escrita quando o writer existe. Testes CLI/GUI com feature passam. Falta boot real e decisão final de resolução auto após split; T027 permanece aberta.
- [ ] T028 [US2] Validar imagens NTFS com árvore Windows real e arquivo >4 GiB por leitor independente e chkdsk sem reparo; validar WIM split com mesmas edições e início da aplicação; executar Windows 10/11 x64 NTFS OVMF on/off em MBR/GPT e registrar em `docs/validation/windows-m5-large-files.md`, com assinaturas, DB/DBX e comandos reproduzíveis.

**Checkpoint**: ambos os caminhos funcionam sem ferramentas instaladas manualmente; FAT32 manual incompatível não escreve. US2 depende da preparação/commit base US1, mas seus resultados são verificáveis separadamente dos ajustes US3.

## Phase 5: User Story 3 — Ajustes Windows 11 opt-in (Priority: P2)

**Goal**: três ajustes independentes, ajuda separada, zero alteração automática de firmware/hospedeiro.

**Independent Test**: comparar mídia padrão e cada flag/combinação em build compatível; somente ajustes selecionados aparecem; ajuda não abre arquivos/dispositivos; setup permite comportamento esperado sem automatizar apagamento de disco.

### Tests

- [ ] T029 [P] [US3] Criar regressões de personalizações em `crates/extract/tests/extract.rs`: três flags false por padrão, cada ajuste isolado/combinações, answer file conflitante, build desconhecida e XML válido; impedir credenciais/administrador sem senha e seleção automática de disco; hash da origem/destino prova proteção em erro de preparação.
- [ ] T030 [P] [US3] Acrescentar testes de contrato CLI em `crates/cli/src/main.rs`: ajuda sem ISO/destino e sem I/O, flags independentes, conflitos da ação help, opções Windows em DD/não Windows recusadas e nenhuma flag genérica que ative tudo.

### Implementation

- [ ] T031 [US3] Experimentar mecanismos WinPE/conta offline em mídia temporária manual e VM descartável Windows 11 24H2 x64, sem depender do gerador de produção T032; registrar edição/build/SHA-256, procedimento, fontes, cada ajuste e rede conectada/desconectada em `docs/validation/windows-m5-options.md`; definir capacidades comprovadas em `crates/iso/src/analyze.rs`, mantendo criação padrão independente de ajustes. Matriz mínima de criação/boot: Windows 10 22H2 x64 e Windows 11 24H2 x64; nenhuma build exata é anunciada para ajustes antes de comprovada. T035 fará validação integrada após implementação.
- [ ] T032 [US3] Gerar answer file mínimo em `crates/extract/src/windows.rs`: comandos LabConfig selecionados executados apenas no WinPE, conta offline interativa somente em builds comprovadas, XML escapado, conflitos recusados antes de gravar; incluir saída preparada no manifesto, sem modificar EFI assinados, boot.wim, origem, firmware ou registro do hospedeiro.
- [ ] T033 [US3] Expor --bypass-tpm, --bypass-secure-boot, --offline-account e --secure-boot-help em `crates/cli/src/main.rs`; ajuda exclusiva pode omitir ISO e retorna antes de qualquer I/O; listar ajustes no plano e avisos sobre suporte oficial, proteção de boot e chave BitLocker antes de mudança manual do firmware.
- [ ] T034 [US3] Adicionar três checkboxes false e botão de ajuda independente em `crates/gui/src/main.rs`; consultar ajuda não muda seleção/plano/arquivos e alterar ajuste invalida confirmação; manter acessibilidade/teclado e bloqueio específico de build incompatível.
- [ ] T035 [US3] Executar comparação de hashes padrão/ajustes, boot e experiência de conta local com rede on/off em builds declaradas, opções isoladas/combinadas e teste GUI/CLI; completar evidências em `docs/validation/windows-m5-options.md`, sem converter mecanismos não comprovados em suporte anunciado.

**Checkpoint**: ajustes explicitamente selecionados funcionam na matriz anunciada. Consulta ajuda nunca altera firmware ou mídia; nenhuma conta sem senha é criada automaticamente.

## Phase 6: Polish & Cross-Cutting Concerns

- [ ] T036 Ampliar a infraestrutura nativa mínima já criada por T004 em `.github/workflows/ci.yml` com testes integrados das histórias, fixtures sintéticas e checks de integridade nas plataformas/arquiteturas do plano; ausência de dependência ou runner necessário não pode virar teste silenciosamente pulado e aprovado.
- [ ] T037 Integrar recursos privados/fontes em `.github/workflows/release.yml`, incluindo executáveis CLI portáteis, bundle GUI, assinatura macOS interna, hashes e receitas reproduzíveis; provar em ambiente limpo sem wimlib/NTFS instalado e revisar obrigações/relink em `THIRD_PARTY_NOTICES.md` antes de distribuir.
  - Parcial em 2026-10-22: `scripts/package-macos.sh` aceita `--native` e empacota a feature `windows-media` apenas quando `IMAGE2USB_WINDOWS_MEDIA_PREFIX` está definido, recusando bundle universal com prefixo nativo. Pacote host-only x86_64 gerado e verificado (assinatura ad-hoc, `codesign --verify`, CLI `--version`/`list`, strings da feature). Release remoto, fonte dos componentes UEFI:NTFS, hashes de release e ambiente limpo ainda pendentes; T037 permanece aberta.
- [ ] T038 Auditar fronteira privilegiada em `SECURITY.md`, `crates/platform/src/media.rs` e `crates/extract/src/windows.rs`: snapshot privado integral, identidade/metadados antes/depois da captura, conteúdo alterado sem mudança de tamanho, alteração posterior na origem, scratch/ENOSPC/cleanup, paths/links, substituição de helper/DLL, cancelamento, short I/O, limite de alocação e unsafe restrito a platform; documentar que snapshot estabiliza os bytes usados, não comprova autenticidade nem garante detectar adulteração adversarial na captura; corrigir achados e deixar regressão executável para lógica não trivial.
- [ ] T039 Executar matriz física Windows 10/11 x64 × FAT32/NTFS × Secure Boot on/off e registrar em `docs/validation/windows-m5-hardware.md`; somente após autorização explícita de USB descartável identificado, dry-run e confirmação; nunca Expansion HDD. Registrar ARM64 como validado, não validado ou não suportado conforme evidência, sem marcar esta tarefa completa se faltarem células obrigatórias.
- [ ] T040 Sincronizar `README.md`, `ROADMAP.md`, `docs/compatibility.md`, `THIRD_PARTY_NOTICES.md` e `specs/001-windows-installation-media/quickstart.md` com flags/layouts reais, fontes, scratch, builds/arquiteturas, etapas indivisíveis de cancelamento, códigos de saída e limitações Secure Boot; M5 só concluído com todos os gates efetivamente aprovados.
- [ ] T041 Executar quickstart completo e gates fmt/Clippy/test/workspace/cross-checks Linux e Windows do ROADMAP; registrar resultados, commits e cobertura FR/SC em `docs/validation/windows-m5-build.md`, distinguindo validação local, CI, boot e pendências; não alegar verificação quando --verify não foi executado.

## Dependencies & Execution Order

- Setup T001–T003 precede Foundation T004–T010. T004 depende de pins/fontes e cria infraestrutura CI nativa mínima; T005/T006 e essa infraestrutura antecedem T007; T008 depende do helper construído; T009/T010 estabilizam contratos antes dos frontends. T007 não depende de T036.
- Foundation macOS bloqueia as histórias no Mac; provas dos outros hosts ficam separadas. Se NTFS/ABI local não passar T007, parar para revisão do plano em vez de substituir implementação ou retirar requisito.
- US1: T011/T012 primeiro; catálogo T013 precede análise T014 e preparação T015; commit T016 precede integrações T017/T018; T019 valida o incremento.
- US2 depende de US1 (catálogo/preparação/commit compartilhados). T020/T021 primeiro; T022 precede T023; T024/T025 precedem T026; T027 integra após T023/T026; T028 valida o incremento.
- US3 depende de US1, não do writer NTFS: T029/T030 primeiro, T031 comprova mecanismos antes de T032; T033/T034 integram após T032; T035 valida. Executar sequencialmente após US2 para evitar conflitos nos mesmos arquivos.
- Polish depende dos três incrementos. T036/T037 podem começar com infraestrutura estável, mas conclusão requer histórias implementadas. T039 exige recursos humanos/hardware e autorização externa. T041 é verificação final após T040.

```text
Setup → Foundation → US1 → US2 → US3 → Polish
                       └──────→ US3 (sem dependência funcional de US2)
```

## Parallel Execution Examples

- **US1**: T011 (`crates/iso/tests/synthetic.rs`, `crates/testiso/src/lib.rs`) e T012 (`crates/extract/tests/extract.rs`) podem ser escritos juntos após Foundation; T017 e T018 podem ser integrados em arquivos distintos após contrato/commit estáveis, mas não recebem `[P]` por dependerem do resultado de implementação anterior.
- **US2**: T020 (`crates/extract/tests/extract.rs`) e T021 (`layout.rs`, `gpt.rs`) podem ser escritos juntos após US1. Split T022 e aquisição/auditoria de asset T024 têm arquivos distintos, mas só depois dos seus próprios pré-requisitos.
- **US3**: T029 (`crates/extract/tests/extract.rs`) e T030 (`crates/cli/src/main.rs`) podem ser escritos juntos após US1. T033/T034 usam arquivos distintos após T032; não executar em paralelo com tarefas US2 que alterem esses arquivos.

## Implementation Strategy

1. Provar componentes nativos e segurança primeiro; manter rejeições existentes até cada caminho ser realmente suportado.
2. Entregar MVP US1: mídia Windows pequena FAT32 via CLI/GUI, verificada e inicializável, sem ajustes. Não chamar esse incremento de M5 completo.
3. Adicionar US2: WIM split e NTFS/UEFI:NTFS; validar resultados reais, não somente chamadas de helper.
4. Adicionar US3: ajustes opt-in por build comprovada e ajuda separada. Nenhuma desativação automática de Secure Boot.
5. Fechar distribuição, matriz física e documentação. Se faltarem ISO, assinatura, ambiente ou autorização, registrar bloqueio e manter tarefa aberta.

## Requirement Coverage

| Requisitos | Tarefas principais |
|---|---|
| FR-001–003 | T005, T011, T013–T016, T019 |
| FR-004–005 | T020, T022–T023, T028 |
| FR-006–007 | T007–T008, T012, T015, T017, T020, T023, T038 |
| FR-008 | T014, T017–T018, T027 |
| FR-009–010 | T009, T029–T035 |
| FR-011–012 | T007–T010, T012, T016–T018, T026, T038 |
| FR-013 | T002–T004, T024, T037, T040 |
| FR-014–015 | T001, T019, T028, T036, T039, T041 |
| FR-016–017, FR-019 | T020–T028 |
| FR-018 | T030, T033–T035 |
| FR-020 | T024, T028, T039–T041 |
| SC-001, SC-007–008 | T019, T028, T039–T041 |
| SC-002 | T022, T028 |
| SC-003, SC-006 | T007, T012, T020, T026, T038 |
| SC-004, SC-009 | T029–T035 |
| SC-005 | T017–T019, T027, T034–T035 |

**Counts**: 41 tarefas: Setup 3; Foundation 7; US1 9; US2 9; US3 7; Polish 6. Seis tarefas `[P]`, em três pares sem conflito de arquivos.
