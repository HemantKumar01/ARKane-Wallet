"use client"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { toast } from "sonner"
import { Copy } from "lucide-react"
import QRCode from "react-qr-code"

interface ReceiveModalProps {
  isOpen: boolean
  onClose: () => void
  onchainAddress: string
  offchainAddress: string
}

export function ReceiveModal({ isOpen, onClose, onchainAddress, offchainAddress }: ReceiveModalProps) {
  const handleCopy = (address: string, type: string) => {
    navigator.clipboard.writeText(address)
    toast.success("Address Copied", {
      description: `${type} address copied to clipboard`,
    })
  }

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="sm:max-w-md border-zinc-800 bg-zinc-950">
        <DialogHeader>
          <DialogTitle>Receive Bitcoin</DialogTitle>
          <DialogDescription>Share your address to receive Bitcoin</DialogDescription>
        </DialogHeader>
        <Tabs defaultValue="offchain" className="w-full">
          <TabsList className="grid w-full grid-cols-2 mb-6">
            <TabsTrigger value="offchain">Offchain</TabsTrigger>
            <TabsTrigger value="onchain">Onchain</TabsTrigger>
          </TabsList>
          <TabsContent value="offchain" className="flex flex-col items-center space-y-4">
            <div className="p-4 bg-white rounded-lg">
              <QRCode value={offchainAddress} size={180} style={{ maxWidth: "100%", height: "auto" }} />
            </div>
            <div className="w-full p-3 bg-zinc-900 rounded-md flex items-center justify-between">
              <p className="text-xs overflow-hidden text-ellipsis break-all pr-2 max-w-[calc(100%-40px)]">
                {offchainAddress}
              </p>
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8 flex-shrink-0"
                onClick={() => handleCopy(offchainAddress, "Offchain")}
              >
                <Copy className="h-4 w-4" />
                <span className="sr-only">Copy address</span>
              </Button>
            </div>
          </TabsContent>
          <TabsContent value="onchain" className="flex flex-col items-center space-y-4">
            <div className="p-4 bg-white rounded-lg">
              <QRCode value={onchainAddress} size={180} style={{ maxWidth: "100%", height: "auto" }} />
            </div>
            <div className="w-full p-3 bg-zinc-900 rounded-md flex items-center justify-between">
              <p className="text-xs overflow-hidden text-ellipsis break-all pr-2 max-w-[calc(100%-40px)]">
                {onchainAddress}
              </p>
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8 flex-shrink-0"
                onClick={() => handleCopy(onchainAddress, "Onchain")}
              >
                <Copy className="h-4 w-4" />
                <span className="sr-only">Copy address</span>
              </Button>
            </div>
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  )
}
