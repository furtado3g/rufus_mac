# Research — M5 Windows installation media

Data: 2026-10-21. Pesquisa upstream e inspeção do repositório; não houve build nativo, inspeção criptográfica de binários ou teste de boot nesta fase. Versões de bibliotecas sem release selecionado abaixo devem ser fixadas com checksum no primeiro trabalho de integração, nunca baixadas em runtime.

## 1. Leitura UDF e tamanhos grandes

**Decision**: integrar libudfread por API segura em platform, usando callbacks read-only para arquivo Rust; ISO mantém catálogo compartilhado com tamanhos/extents u64. Preferir árvore UDF nas mídias Windows UDF; preservar ISO9660/Joliet Linux. Implementar multi-extent ISO9660 com validação, não apenas cast u32 para u64.

**Rationale**: leitor atual usa extent/tamanho simples de 32 bits e rejeita UDF-only. libudfread tem tamanho/seek int64 e leitura em blocos. Callback resolve caminhos Unicode sem conversões C específicas por host.

**Alternatives considered**: libarchive não fornece leitor UDF equivalente; montagem por SO exige privilégios/APIs divergentes; parser UDF próprio aumenta superfície de entrada não confiável.

**Evidence**: [API libudfread, mirror consultado](https://github.com/firecore/libudfread/blob/master/src/udfread.h), [upstream VideoLAN](https://code.videolan.org/videolan/libudfread), [libarchive readers](https://github.com/libarchive/libarchive/blob/master/libarchive/archive_read_support_format_all.c).

**Gate**: obter release/commit do upstream primário, auditar licença LGPL-2.1-or-later e compilar todos os hosts; testar arquivos fragmentados, limites, 4 GiB e UDF Windows reais antes de alegar suporte.

## 2. WIM: helper entregue, não ferramenta instalada

**Decision**: entregar wimlib-imagex 1.14.5 com dependências privadas, sem FUSE/NTFS-3G; chamada por caminho absoluto com argumentos, sem shell. Empacotar no bundle ou incorporar/extrair recurso verificado em diretório privado no executável portátil. Fonte e receita exatas acompanham a distribuição.

**Rationale**: evita ABI FFI adicional; Windows upstream usa MinGW, não MSVC. Helper é GPL-3.0-or-later; libwim isolada tem opção LGPL, não uma exceção geral de static linking. O projeto já é GPL-3.0-or-later.

**Alternatives considered**: libwim compartilhada permite substituição LGPL mas amplia gestão ABI; static FFI exige relink quando usado sob LGPL; exigir instalação manual contraria o produto.

**Evidence**: [README](https://github.com/ebiggers/wimlib/blob/master/README.md), [Windows](https://github.com/ebiggers/wimlib/blob/master/README.WINDOWS.md), [COPYING](https://github.com/ebiggers/wimlib/blob/master/COPYING).

**Gate**: builds nativos, assinatura de recursos macOS, suporte Windows ARM64 e dependências auditados. Nenhuma carga de binário por PATH, diretório gravável compartilhado ou download automático.

## 3. Divisão WIM

**Decision**: preparar `wimlib-imagex split install.wim install.swm 3800`; enumerar e verificar cada parte efetiva, metadados, sequência e imagens. Recurso individual pode superar limite pedido: auto resolve para NTFS com motivo; FAT32 manual falha antes de escrever. Solid WIM não suportado pela divisão direta segue a mesma regra. Não converter ESD neste M5.

**Rationale**: máximo FAT32 é 4.294.967.295 bytes. Tamanho solicitado não é garantia: recursos comprimidos não podem atravessar partes. Não prometer divisão universal.

**Alternatives considered**: export/recompress de solid ESD/WIM custa scratch/tempo e não resolve todo recurso enorme; copiar até falhar destruiria destino sem resultado utilizável.

**Evidence**: [wimsplit](https://man.archlinux.org/man/wimsplit.1.en), [split.c](https://github.com/ebiggers/wimlib/blob/master/src/split.c), [Microsoft split Windows image](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/split-a-windows-image--wim--file-to-span-across-multiple-dvds?view=windows-11).

## 4. Escrita NTFS portátil

**Decision**: libntfs-3g com device operations adaptadas a RawIo/PartitionIo, shim C limitado e formatter mkntfs adaptado. Sem FUSE, mounts ou ferramentas do host. Serializar chamadas nativas se formatter usar estado global. Unsafe somente em platform; callbacks capturam erros, nunca deixam panic atravessar C.

**Rationale**: implementação madura de metadados NTFS; stdlib e dependências existentes não oferecem escrita NTFS portátil. mkntfs é código de utilitário, não API de formatação pronta: a adaptação é trabalho real e prova inicial bloqueante.

**Alternatives considered**: crate `ntfs` é read-only. `rust-fs-ntfs` tem escrita/formatter, mas declara estágio experimental e limites de MFT/diretórios: não escolhido para ferramenta destrutiva. Formatação nativa macOS não cobre NTFS gravável; writer próprio rejeitado.

**Evidence**: [NTFS-3G](https://github.com/tuxera/ntfs-3g), [device operations](https://github.com/tuxera/ntfs-3g/blob/edge/include/ntfs-3g/device.h), [mkntfs](https://github.com/tuxera/ntfs-3g/blob/edge/ntfsprogs/mkntfs.c), [read-only ntfs](https://github.com/ColinFinck/ntfs), [alternativa experimental](https://github.com/antimatter-studios/rust-fs-ntfs).

**Gate**: fixar código GPLv2-or-later compatível com distribuição GPLv3, auditar arquivos individuais, compilar bibliotecas em MSVC/MinGW com ABI comprovada ou port mínimo. Provar formatter via callbacks, arquivo >4 GiB, diretórios reais Windows e chkdsk. Se falhar, parar para revisão; nenhum fallback experimental implícito.

## 5. UEFI:NTFS e Secure Boot

**Decision**: entregar imagem FAT upstream assinada intacta, fixada por commit/SHA-256, com loader e driver NTFS assinados para x64/ARM64. Layout NTFS principal + FAT auxiliar final em MBR/GPT. Dimensionar partição auxiliar a partir da imagem real, sem hardcode de versão futura.

**Rationale**: firmware lê FAT, carrega driver NTFS e inicia fallback Microsoft no NTFS. Host NTFS writer e driver EFI read-only são componentes independentes. Reconstruir EFI removeria assinatura Microsoft.

**Alternatives considered**: NTFS sem auxiliar depende de firmware; FAT32-only foi explicitamente recusado pelo usuário; recompilar EFI e prometer Secure Boot sem assinatura não funciona.

**Evidence**: [Rufus asset manifest](https://github.com/pbatard/rufus/blob/master/res/uefi/readme.txt), [imagem](https://github.com/pbatard/rufus/blob/master/res/uefi/uefi-ntfs.img), [UEFI:NTFS v2.8](https://github.com/pbatard/uefi-ntfs/releases/tag/v2.8), [trust constraints](https://github.com/pbatard/uefi-ntfs#secure-boot-compatibility), [Microsoft revocations](https://support.microsoft.com/help/5025885).

**Gate**: inspecionar conteúdo, PE signatures e hashes do asset escolhido; documentar licença por componente (UEFI:NTFS GPLv2, NTFS driver e eventuais EfiFs conforme arquivos reais). Hospedar fontes correspondentes. Certificados third-party UEFI, DBX, Windows boot manager e transição de CAs importam; assinatura presente não prova aceitação em todo firmware. Testar on/off nos dois caminhos conforme SC-008, não reduzir requisito para contornar revogação.

## 6. Ajustes Windows 11 sem alteração do hospedeiro

**Decision**: gerar answer file mínimo no destino preparado; comandos `reg add` executados pelo WinPE configuram separadamente LabConfig BypassTPMCheck/BypassSecureBootCheck. Opção offline solicita o fluxo interativo local em builds comprovados; usar ajuste de registro compatível com a build (como BypassNRO somente onde comprovadamente funciona), não criar conta administrativa sem senha nem armazenar credenciais. Não selecionar/apagar disco automaticamente. Rejeitar build/edição não comprovada para o ajuste solicitado e conflitos com answer files existentes antes de gravar.

**Rationale**: reaproveita mecanismos do instalador, evita hive writer e alteração boot.wim. Bypass de requisito não altera firmware. Conta offline e comandos variam por build; não há garantia futura.

**Alternatives considered**: edição offline de hive exige ferramenta extra; conta pré-criada exige política de senha e credenciais fora do escopo; prometer BypassNRO universal contradiz mudanças Microsoft.

**Evidence**: [Rufus wue.c](https://github.com/pbatard/rufus/blob/master/src/wue.c), [relato 25H2](https://github.com/pbatard/rufus/issues/2810), [Microsoft Insider 26220.6772](https://blogs.windows.com/windows-insider/2025/10/06/announcing-windows-11-insider-preview-build-26220-6772-dev-channel/).

**Gate**: escolher e registrar builds ISO legais concretas no início dos testes, testar cada opção isolada e combinada, rede conectada/desconectada. Suporte de criação padrão não depende do suporte aos ajustes dessa build. Se nenhum mecanismo permitir conta local nas builds alvo, isso bloqueia FR-009 nessa matriz e exige revisão, não sucesso declarado.

## 7. Temporários, integridade e falhas

**Decision**: tempfile já instalado, diretórios privados e ACLs adequadas, arquivos criados exclusivamente, catálogo limitado e manifestos SHA-256. Capturar snapshot privado integral antes da análise definitiva; preservar handle origem read-only e comparar identidade/metadados antes/depois da captura, abortando mudança detectada. Leituras posteriores usam somente snapshot; scratch inclui ISO inteira e transformações. Mudança posterior na origem não afeta o plano. Snapshot estabiliza bytes usados, não comprova autenticidade nem garante detecção adversarial durante captura. Helpers só recebem caminhos privados controlados e nunca dispositivo real. Callbacks NTFS são a única escrita no destino pelo fluxo autorizado.

**Rationale**: garante falhas determinísticas de preparação antes de apagar; rejeita symlink/hard link entre saída e ISO. Scratch necessário deve ser exibido e sua disponibilidade validada; falha real de ENOSPC ainda tratada sem tocar destino.

**Alternatives considered**: streaming direto de transformações para USB economiza scratch mas impede garantir partes válidas antes da destruição; diretórios compartilhados aumentam risco de substituição privilegiada.

## Research completion

Decisões técnicas tomadas; nenhum `NEEDS CLARIFICATION` aberto. Gates de execução não foram confundidos com resultados: bibliotecas nativas, assinaturas, ajustes por build e hardware ainda precisam ser comprovados durante implementação. Nenhuma alegação de M5 concluído.
