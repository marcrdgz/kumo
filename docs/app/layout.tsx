import { RootProvider } from 'fumadocs-ui/provider/next';
import { Inter, JetBrains_Mono, Martian_Mono } from 'next/font/google';
import { OG_IMAGE, SITE_URL } from '@/lib/site';
import './global.css';

const inter = Inter({
  subsets: ['latin'],
  variable: '--font-inter',
});

// Display face for landing headlines: terminal-native, wide and deliberate.
const display = Martian_Mono({
  subsets: ['latin'],
  variable: '--font-martian-mono',
});

const mono = JetBrains_Mono({
  subsets: ['latin'],
  variable: '--font-jetbrains-mono',
});

export const metadata = {
  metadataBase: new URL(SITE_URL),
  openGraph: { siteName: 'kumo', type: 'website', images: [OG_IMAGE] },
  twitter: { card: 'summary_large_image' },
};

export default function Layout({ children }: LayoutProps<'/'>) {
  return (
    <html
      lang="en"
      className={`${inter.variable} ${mono.variable} ${display.variable}`}
      suppressHydrationWarning
    >
      <body className="flex flex-col min-h-screen">
        <RootProvider
          search={{
            options: {
              type: 'static',
              api: `${process.env.NEXT_PUBLIC_BASE_PATH ?? ''}/api/search`,
            },
          }}
        >
          {children}
        </RootProvider>
      </body>
    </html>
  );
}
