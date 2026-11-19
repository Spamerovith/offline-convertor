/* Interface language: Russian (source) and English.
 * The interface and the engine write Russian text; in English mode every text node and the
 * title / aria-label / placeholder attributes are translated as they appear on screen
 * (exact phrase → pattern → parts split by « · »). Switching back restores the originals,
 * so the language changes instantly without restarting or losing the queue.
 * Elements with translate="no" (file names, recognised text) are never touched. */
(function () {
  const LANGS = [['ru', 'Русский'], ['en', 'English']];

  const EN = {
    // Sizes
    'Б': 'B', 'КБ': 'KB', 'МБ': 'MB', 'ГБ': 'GB', 'ТБ': 'TB',
    // Navigation and sections
    'Разделы': 'Sections', 'Сжатие': 'Compress', 'Конвертация': 'Convert', 'Распознавание текста': 'Text recognition',
    'Пакетное сжатие': 'Batch compression', 'Видео': 'Video', 'Изображения': 'Images', 'Аудио': 'Audio', 'PDF': 'PDF',
    'Сжатие видео': 'Compress video', 'Сжатие изображений': 'Compress images', 'Сжатие аудио': 'Compress audio', 'Сжатие PDF': 'Compress PDF',
    'Видео-конвертер': 'Video converter', 'Аудио-конвертер': 'Audio converter', 'Конвертер изображений': 'Image converter',
    'Изображение в текст': 'Image to text', 'Сканировать в текст': 'Scan to text', 'Скан в текст': 'Scan to text', 'PDF в Word': 'PDF to Word',
    'Настройки': 'Settings', 'Закрыть': 'Close',
    // Formats lines
    'Видео, фото, аудио и PDF': 'Video, photos, audio and PDF',
    'MP4, MKV, MOV, AVI, WEBM и другие': 'MP4, MKV, MOV, AVI, WEBM and more',
    'В MP4, MKV, WEBM, MOV, AVI, FLV, WMV, MPEG, 3GP, TS, M4V, OGV, GIF': 'To MP4, MKV, WEBM, MOV, AVI, FLV, WMV, MPEG, 3GP, TS, M4V, OGV, GIF',
    'В MP3, AAC, M4A, OPUS, OGG, FLAC, WAV, AIFF, ALAC, WMA, AC3 · звук из видео': 'To MP3, AAC, M4A, OPUS, OGG, FLAC, WAV, AIFF, ALAC, WMA, AC3 · audio from video',
    'В JPG, PNG, WEBP, AVIF, JXL, GIF, BMP, TIFF, ICO, TGA · из HEIC и PSD тоже': 'To JPG, PNG, WEBP, AVIF, JXL, GIF, BMP, TIFF, ICO, TGA · from HEIC and PSD too',
    'JPG, PNG, WEBP, AVIF, HEIC, BMP, TIFF': 'JPG, PNG, WEBP, AVIF, HEIC, BMP, TIFF',
    'MP3, WAV, FLAC, M4A, OGG, OPUS, WMA': 'MP3, WAV, FLAC, M4A, OGG, OPUS, WMA',
    'JPG, PNG, WEBP, HEIC, BMP, TIFF': 'JPG, PNG, WEBP, HEIC, BMP, TIFF',
    'Сканы и фото документов, PDF-сканы': 'Scans and photos of documents, scanned PDF',
    // Drop zone and queue
    'Перетащите файлы или папки сюда': 'Drop files or folders here', 'Исходные файлы не изменяются': 'Source files are never changed',
    'Отпустите, чтобы добавить': 'Release to add', 'Выбрать файлы': 'Choose files', 'Папку': 'Folder',
    'Загрузить всё': 'Save all', 'Убрать сохранённые': 'Remove saved', 'Очистить': 'Clear', 'Отменить всё': 'Cancel all',
    'Улучшение': 'Enhance', 'Мягкое шумоподавление и резкость': 'Gentle denoise and sharpening',
    'Дополнительные настройки': 'Settings', 'Начать': 'Start', 'Обработать заново': 'Process again', 'Готово': 'Done',
    'Добавьте файлы': 'Add files', 'Добавьте файлы для обработки': 'Add files to process',
    'Готов к обработке': 'Ready', 'Отменено': 'Cancelled', 'Ошибка': 'Error', 'В очереди': 'In queue', 'Запуск': 'Starting',
    'Убрать из списка': 'Remove from list', 'Удалить результат': 'Delete result', 'Отменить': 'Cancel', 'Показать текст': 'Show text',
    'Копировать текст': 'Copy text', 'Открыть файл': 'Open file', 'Показать в папке': 'Show in folder', 'Загрузить на диск': 'Save to disk',
    'Текст скопирован': 'Text copied', 'Не удалось скопировать': 'Could not copy', 'Эти файлы уже в списке': 'These files are already in the list',
    // Compression settings
    'видео': 'video', 'фото': 'photos', 'аудио': 'audio',
    'Качество': 'Quality', 'Качество:': 'Quality:', 'Качество изображений:': 'Image quality:', 'Чем выше качество, тем больше файл': 'Higher quality means a bigger file',
    'Лучшая компрессия': 'Smaller file', 'Лучшее качество': 'Better quality',
    'Метод сжатия': 'Compression method', 'По качеству': 'By quality', 'По размеру': 'By size', 'По битрейту': 'By bitrate',
    'Степень сжатия': 'Compression level', 'Слабая': 'Light', 'Оптимальная': 'Optimal', 'Сильная': 'Strong',
    'Наивысшее качество': 'Highest quality', 'Баланс качества и размера': 'Balance of quality and size', 'Наименьший размер': 'Smallest size',
    'Наименьший размер, подходит для речи': 'Smallest size, fine for speech',
    'Остальные параметры берутся из разделов «Сжатие»': 'Other options come from the «Compress» sections',
    'Установить размер файла:': 'Target file size:', 'Изменить битрейт видео:': 'Video bitrate:', 'Изменить битрейт аудио:': 'Audio bitrate:',
    'Размер каждого файла вместе со звуком. Если он слишком мал для разрешения видео, разрешение будет снижено': 'Size of each file including audio. If it is too small for the video resolution, the resolution is lowered',
    'Ожидаемое качество:': 'Expected quality:', 'Как у исходника': 'Same as source', 'Отличное': 'Excellent', 'Хорошее': 'Good', 'Среднее': 'Fair', 'Заметные потери': 'Visible loss',
    'Файл уже меньше заданного размера': 'The file is already smaller than this', 'Файлы уже меньше заданного размера': 'The files are already smaller than this',
    'Разрешение': 'Resolution', 'Частота кадров': 'Frame rate', 'Контейнер': 'Container', 'Звук': 'Audio', 'Авто': 'Auto', 'Без звука': 'No audio',
    'Скорость кодирования': 'Encoding speed', 'Стандартная': 'Standard', 'Медленная, лучшее сжатие': 'Slow, best compression', 'Быстрая': 'Fast',
    'Ускорение': 'Acceleration', 'Видеокарта': 'Graphics card', 'Только процессор': 'CPU only',
    '60 кадров/с': '60 fps', '50 кадров/с': '50 fps', '30 кадров/с': '30 fps', '25 кадров/с': '25 fps', '24 кадра/с': '24 fps',
    'Формат результата': 'Output format', 'Максимальная сторона': 'Longest side', 'Сохранять EXIF и цветовой профиль': 'Keep EXIF and color profile',
    'Качество изображений': 'Image quality', 'Текст и векторная графика не изменяются': 'Text and vector graphics stay unchanged',
    'Разрешение изображений': 'Image resolution', 'Без изменений': 'Unchanged', 'Перевести изображения в оттенки серого': 'Convert images to grayscale',
    'Конвертировать в': 'Convert to', 'Видеокодек': 'Video codec', 'Без перекодирования': 'No re-encoding',
    'Кадров в секунду (GIF)': 'Frames per second (GIF)', 'Ширина GIF': 'GIF width', 'Обрезка': 'Trim',
    'Начало 0:00:00': 'Start 0:00:00', 'Конец 0:00:00': 'End 0:00:00', 'Начало, чч:мм:сс': 'Start, hh:mm:ss', 'Конец, чч:мм:сс': 'End, hh:mm:ss',
    'Время указывается как чч:мм:сс, мм:сс или в секундах': 'Enter time as hh:mm:ss, mm:ss or seconds',
    'Битрейт': 'Bitrate', 'Разрядность': 'Bit depth', '16 бит': '16-bit', '24 бит': '24-bit', 'Частота дискретизации': 'Sample rate',
    '48 000 Гц': '48,000 Hz', '44 100 Гц': '44,100 Hz', '32 000 Гц': '32,000 Hz', '22 050 Гц': '22,050 Hz',
    'Каналы': 'Channels', 'Стерео': 'Stereo', 'Моно': 'Mono', 'Выровнять громкость': 'Normalize volume', 'Фон вместо прозрачности': 'Background for transparency',
    // Codec card
    'Видеокарта не найдена, кодирование на процессоре': 'No graphics card found, encoding on the CPU',
    'Проверяю видеокарту…': 'Checking the graphics card…', 'Выбрать другой': 'Choose another', 'Вернуть рекомендуемый': 'Back to recommended',
    'видеокарта': 'graphics card', 'процессор': 'CPU',
    // OCR
    'Язык текста': 'Text language', 'Авто (русский + английский)': 'Auto (Russian + English)', 'Сохранить как': 'Save as', 'Текст (TXT)': 'Text (TXT)',
    'Улучшить контраст перед распознаванием': 'Boost contrast before recognition', 'Улучшить скан': 'Enhance scan',
    'Вставить изображение страницы в Word': 'Insert the page image into Word', 'Способ': 'Method', 'Распознавать каждую страницу': 'Recognize every page',
    'Только текстовый слой': 'Text layer only', 'Вставить изображение страницы': 'Insert the page image',
    // Output
    'Сохранять': 'Save', 'По кнопке «Загрузить на диск»': 'With the «Save to disk» button', 'Сразу рядом с исходными файлами': 'Right away, next to the source files',
    'Сразу в выбранную папку': 'Right away, to the chosen folder', 'Папка для готовых файлов': 'Folder for finished files', 'Изменить': 'Change',
    'Готовые файлы ждут во временной папке и попадают на диск только по кнопке. Несохранённые удаляются при выходе': 'Finished files wait in a temporary folder and are written to disk only with the button. Unsaved files are deleted on exit',
    'Нельзя заменить исходный файл, выберите другое имя': 'The source file can’t be replaced, choose another name',
    'Сохранение…': 'Saving…', 'Сохранено': 'Saved', 'Куда загрузить готовые файлы': 'Where to save the finished files',
    'Выберите папки': 'Choose folders', 'Выберите файлы': 'Choose files', 'Все файлы': 'All files',
    'Очистить список': 'Clear the list', 'Удалить': 'Delete', 'Отмена': 'Cancel',
    'Выйти без сохранения?': 'Quit without saving?', 'Выйти': 'Quit', 'Остаться': 'Stay',
    // Settings dialog
    'Язык': 'Language', 'Тема': 'Theme', 'Графит': 'Graphite', 'Полночь': 'Midnight', 'Изумруд': 'Emerald', 'Янтарь': 'Amber', 'Светлая': 'Light',
    'Обработка': 'Processing', 'Видео одновременно': 'Videos at once', 'Рекомендуется 2': '2 is recommended',
    'Потоки процессора для фото, аудио и PDF': 'CPU threads for photos, audio and PDF', 'Открывать папку после завершения': 'Open the folder when done',
    'Видеокарты': 'Graphics cards', 'Определение…': 'Detecting…', 'Кодировать видео на': 'Encode video on', 'Видеокарте (автоматически)': 'Graphics card (automatic)',
    'Аппаратный кодировщик не найден - видео кодируется на процессоре': 'No hardware encoder found, video is encoded on the CPU',
    'запасной путь для любой видеокарты': 'fallback for any graphics card', '10 бит': '10-bit', '2 блока NVENC': '2 NVENC engines',
    'О программе': 'About', 'Версия': 'Version', 'Движок': 'Engine', 'ffmpeg не найден': 'ffmpeg not found', 'языки не установлены': 'no languages installed',
    'Кодирование на процессоре': 'Encoding on the CPU', 'Сведения о видеокарте': 'Graphics card details', 'Определение видеокарты…': 'Detecting graphics card…',
    'ffmpeg не найден, видео и аудио недоступны': 'ffmpeg not found, video and audio are unavailable',
    // Updates and language prompt
    'Обновить': 'Update', 'Доступна новая версия': 'A new version is available', 'Скачать и установить': 'Download and install', 'Позже': 'Later',
    'Скачивание обновления': 'Downloading the update', 'Не удалось обновить': 'Update failed', 'Нет доступа к сети': 'No network access',
    'Настройки и файлы не изменятся': 'Your settings and files stay as they are',
    'Выберите язык': 'Choose a language',
    // Engine: stages
    'Анализ файла': 'Analyzing', 'Чтение': 'Reading', 'Кодирование': 'Encoding', 'Подбор качества': 'Finding the best quality',
    'Кодирование на видеокарте': 'Encoding on the graphics card', 'Кодирование на видеокарте (Media Foundation)': 'Encoding on the graphics card (Media Foundation)',
    'Уточнение размера': 'Adjusting the size', 'Повторное сжатие': 'Compressing again', 'Чтение PDF': 'Reading PDF', 'Сжатие потоков': 'Compressing streams',
    'Подготовка распознавания': 'Preparing recognition', 'Чтение изображения': 'Reading image', 'Распознавание': 'Recognizing', 'Сохранение': 'Saving',
    // Engine: notes and errors
    'улучшено': 'enhanced', 'улучшение не применено': 'enhancement not applied', 'без перекодирования': 'no re-encoding',
    'PNG без потерь': 'lossless PNG', 'PNG · палитра': 'PNG · palette', 'Сильнее сжать нельзя, сохранена копия': 'Can’t be compressed further, a copy was saved',
    'файл уже меньше заданного размера': 'the file is already smaller than the target',
    'Текст не найден': 'No text found', 'Файл не найден': 'File not found', 'Этот тип файла не поддерживается': 'This file type is not supported',
    'Внутренняя ошибка при обработке файла': 'Internal error while processing the file', 'Временный файл не найден': 'Temporary file not found',
    'Временный файл не найден, обработайте файл ещё раз': 'Temporary file not found, process the file again',
    'В файле нет звуковой дорожки': 'The file has no audio track', 'В файле нет видеодорожки': 'The file has no video track',
    'Ошибка кодирования': 'Encoding error', 'Не удалось перекодировать видео': 'Could not re-encode the video', 'PDF защищён паролем': 'The PDF is password protected',
    'Сбой JPEG-кодировщика': 'JPEG encoder failure', 'Неизвестный формат': 'Unknown format', 'неизвестный формат': 'unknown format',
    'AVIF с прозрачностью не поддерживается, выберите WebP или PNG': 'AVIF with transparency is not supported, choose WebP or PNG',
    'Не удалось подобрать имя файла': 'Could not pick a file name', 'Ошибка фильтра': 'Filter error',
    'Не удалось открыть PDF (возможно, он защищён паролем)': 'Could not open the PDF (it may be password protected)',
    'Файл обновления скачан не полностью': 'The update was not downloaded completely', 'Неверный адрес обновления': 'Invalid update address',
    'Распознавание текста недоступно: в Windows не установлен языковой пакет OCR. Параметры - Время и язык - Язык: добавьте русский или английский язык': 'Text recognition is unavailable: no Windows OCR language pack is installed. Settings - Time & language - Language: add Russian or English',
    'Языки распознавания Windows не найдены: добавьте язык в Параметры → Время и язык → Язык': 'No Windows recognition languages found: add one in Settings → Time & language → Language',
  };

  const num = (s) => s.replace(/(\d)\s(?=\d{3}\b)/g, '$1,').replace(/(\d),(\d)/g, '$1.$2');
  const unit = { 'Б': 'B', 'КБ': 'KB', 'МБ': 'MB', 'ГБ': 'GB', 'ТБ': 'TB' };
  const files = (n) => `${n} ${Number(n) === 1 ? 'file' : 'files'}`;
  const P = [
    [/^([\d\s,.]+) (Б|КБ|МБ|ГБ|ТБ)$/, (m, n, u) => `${num(n)} ${unit[u]}`],
    [/^([\d\s,.]+) (Б|КБ|МБ|ГБ|ТБ) → ([\d\s,.]+) (Б|КБ|МБ|ГБ|ТБ)$/, (m, a, u, b, v) => `${num(a)} ${unit[u]} → ${num(b)} ${unit[v]}`],
    [/^сэкономлено ([\d\s,.]+) (Б|КБ|МБ|ГБ|ТБ)$/, (m, n, u) => `saved ${num(n)} ${unit[u]}`],
    [/^(\d+) кбит\/с$/, (m, n) => `${n} kbps`],
    [/^(\d+) (файл|файла|файлов)$/, (m, n) => files(n)],
    [/^Загрузить всё \((\d+)\)$/, (m, n) => `Save all (${n})`],
    [/^В работе: (\d+) из (\d+)$/, (m, a, b) => `Working: ${a} of ${b}`],
    [/^К обработке: (\d+) (файл|файла|файлов)$/, (m, n) => `To process: ${files(n)}`],
    [/^Кодек: (.+) - рекомендуемый$/, (m, c) => `Codec: ${c}, recommended`],
    [/^Кодек: (.+)$/, (m, c) => `Codec: ${c}`],
    [/^Принять (.+)$/, (m, c) => `Use ${c}`],
    [/^Рекомендуется (.+?): (.+)$/, (m, c, r) => `Recommended: ${c}. ${tr(r)}`],
    [/^Рекомендуемый кодек: (.*)$/, (m, c) => `Recommended codec: ${c}`],
    [/^Кодирование на (.+)\. Самый компактный кодек$/, (m, g) => `Encoding on ${g}. The most compact codec`],
    [/^Кодирование на (.+)\. AV1 эта видеокарта не поддерживает$/, (m, g) => `Encoding on ${g}. This card does not support AV1`],
    [/^Кодирование на (.+)$/, (m, g) => `Encoding on ${g}`],
    [/^Кодирование$/, () => 'Encoding'],
    [/^(\d+(?:[.,]\d+)?)×$/, (m, x) => `${x.replace(',', '.')}×`],
    [/^(\d+)%$/, (m, x) => `${x}%`],
    [/^Разрешение будет снижено до (\d+)p$/, (m, h) => `Resolution will be lowered to ${h}p`],
    [/^разрешение снижено до (\d+)p$/, (m, h) => `resolution lowered to ${h}p`],
    [/^Хуже всего у «(.+)»$/, (m, n) => `Worst: «${n}»`],
    [/^JPEG · качество (\d+)$/, (m, q) => `JPEG · quality ${q}`],
    [/^качество (\d+)$/, (m, q) => `quality ${q}`],
    [/^Изображений: (\d+)$/, (m, n) => `Images: ${n}`],
    [/^Пережато изображений: (\d+)$/, (m, n) => `Images recompressed: ${n}`],
    [/^Страница (\d+) из (\d+)$/, (m, a, b) => `Page ${a} of ${b}`],
    [/^Текстовый слой PDF$/, () => 'PDF text layer'],
    [/^символов: (\d+)$/, (m, n) => `characters: ${n}`],
    [/^Текстовый слой \+ распознавание$/, () => 'Text layer + recognition'],
    [/^Распознано символов: (\d+)$/, (m, n) => `Characters recognized: ${n}`],
    [/^Сохранено: (.+)$/, (m, f) => `Saved: ${f}`],
    [/^Сохранено (\d+), ошибок (\d+)$/, (m, a, b) => `Saved ${a}, errors ${b}`],
    [/^Сохранено (\d+) (файл|файла|файлов) в (.+)$/, (m, n, f, d) => `Saved ${files(n)} to ${d}`],
    [/^Пропущено (\d+) (файл|файла|файлов): формат не подходит для «(.+)»$/, (m, n, f, t) => `Skipped ${files(n)}: the format doesn’t fit «${tr(t)}»`],
    [/^(.+): готово (\d+)$/, (m, t, n) => `${tr(t)}: done ${n}`],
    [/^(.+): готово (\d+), с ошибкой (\d+)$/, (m, t, n, e) => `${tr(t)}: done ${n}, failed ${e}`],
    [/^нажмите «Загрузить на диск»$/, () => 'press «Save to disk»'],
    [/^Все \((.+)\)$/, (m, n) => `All (${n})`],
    [/^Всех видеокартах сразу \((\d+)\)$/, (m, n) => `All graphics cards at once (${n})`],
    [/^драйвер (.+)$/, (m, d) => `driver ${d}`],
    [/^(\d+) видеокарты$/, (m, n) => `${n} graphics cards`],
    [/^(.+) на GPU$/, (m, c) => `${c} on GPU`],
    [/^(\d+) (?:готовый файл не сохранён|готовых файла не сохранены|готовых файлов не сохранены) на диск и будут удалены$/, (m, n) => `${files(n)} not saved to disk will be deleted`],
    [/^(\d+) (?:готовый файл не сохранён|готовых файла не сохранены|готовых файлов не сохранены) на диск\. При выходе (?:он будет удалён|они будут удалены)$/, (m, n) => `${files(n)} not saved to disk. ${Number(n) === 1 ? 'It' : 'They'} will be deleted on exit`],
    [/^Доступна версия (.+)$/, (m, v) => `Version ${v} is available`],
    [/^Обновить до (.+)$/, (m, v) => `Update to ${v}`],
    [/^Не удалось обновить: (.+)$/, (m, e) => `Update failed: ${tr(e)}`],
    [/^Формат (.+) не поддерживается$/, (m, f) => `Format ${f} is not supported`],
    [/^Ошибка кодирования: (.*)$/s, (m, e) => `Encoding error: ${e}`],
    [/^Ошибка фильтра: (.*)$/s, (m, e) => `Filter error: ${e}`],
    [/^Файл не распознан: (.*)$/s, (m, e) => `File not recognized: ${e}`],
    [/^Не удалось (открыть PDF|сохранить PDF|прочитать файл|записать файл|прочитать изображение|сохранить|запустить ffmpeg|запустить движок ffmpeg|создать папку вывода): (.*)$/s,
      (m, w, e) => `Could not ${{ 'открыть PDF': 'open the PDF', 'сохранить PDF': 'save the PDF', 'прочитать файл': 'read the file', 'записать файл': 'write the file', 'прочитать изображение': 'read the image', 'сохранить': 'save', 'запустить ffmpeg': 'start ffmpeg', 'запустить движок ffmpeg': 'start ffmpeg', 'создать папку вывода': 'create the output folder' }[w]}: ${e}`],
    [/^Нет доступа к (папке вывода|папке|папке программы): (.*)$/s, (m, w, e) => `No access to the ${w === 'папке программы' ? 'program folder' : w === 'папке вывода' ? 'output folder' : 'folder'}: ${e}`],
    [/^Не удалось заменить программу: (.*)$/s, (m, e) => `Could not replace the program: ${e}`],
  ];

  let lang = 'ru';
  try { lang = localStorage.getItem('oc-lang') || 'ru'; } catch {}

  function tr(s) {
    if (lang !== 'en' || !s || !/[А-Яа-яЁё]/.test(s)) return s;
    const lead = s.match(/^\s*/)[0], tail = s.match(/\s*$/)[0];
    const core = s.trim();
    if (EN[core] != null) return lead + EN[core] + tail;
    // «A · B · C» (notes, stages with speed and percent): each part on its own
    if (core.includes(' · ')) return lead + core.split(' · ').map(tr).join(' · ') + tail;
    for (const [re, fn] of P) {
      const m = core.match(re);
      if (m) return lead + fn(...m) + tail;
    }
    if (core.includes(', ') && core.split(', ').every((p) => EN[p] != null)) return lead + core.split(', ').map(tr).join(', ') + tail;
    return s;
  }

  // Original (Russian) values, so the language can be switched back and forth.
  const origText = new WeakMap();
  const shownText = new WeakMap();
  const origAttr = new WeakMap();
  const ATTRS = ['title', 'aria-label', 'placeholder'];
  const skip = (node) => {
    const el = node.nodeType === 1 ? node : node.parentElement;
    return !el || !!el.closest('[translate="no"], script, style, textarea');
  };

  function doText(node) {
    if (skip(node)) return;
    const cur = node.data;
    if (shownText.get(node) !== cur) origText.set(node, cur); // new text from the app
    const src = origText.get(node) ?? cur;
    const out = tr(src);
    shownText.set(node, out);
    if (out !== cur) node.data = out;
  }
  function doAttrs(el) {
    if (skip(el)) return;
    let o = origAttr.get(el);
    for (const a of ATTRS) {
      if (!el.hasAttribute(a)) continue;
      const cur = el.getAttribute(a);
      if (!o) { o = {}; origAttr.set(el, o); }
      if (o['_' + a] !== cur) o[a] = cur;
      const out = tr(o[a]);
      o['_' + a] = out;
      if (out !== cur) el.setAttribute(a, out);
    }
  }
  function walk(root) {
    if (root.nodeType === 3) { doText(root); return; }
    if (root.nodeType !== 1) return;
    doAttrs(root);
    const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
    let n;
    while ((n = w.nextNode())) n.nodeType === 3 ? doText(n) : doAttrs(n);
  }

  const mo = new MutationObserver((list) => {
    for (const m of list) {
      if (m.type === 'childList') m.addedNodes.forEach(walk);
      else if (m.type === 'characterData') doText(m.target);
      else if (m.type === 'attributes' && ATTRS.includes(m.attributeName)) doAttrs(m.target);
    }
  });

  function start() {
    walk(document.body);
    mo.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ATTRS });
  }

  function setLang(l) {
    lang = l === 'en' ? 'en' : 'ru';
    try { localStorage.setItem('oc-lang', lang); } catch {}
    document.documentElement.lang = lang;
    document.title = 'Offline Convertor';
    walk(document.body);
  }

  window.I18N = { LANGS, t: tr, setLang, start, get lang() { return lang; } };
  document.documentElement.lang = lang;
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
})();
